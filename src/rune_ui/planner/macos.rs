//! Native presentation and the timer that also runs with Settings closed.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreen, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSDate, NSNumber, NSString, NSTimeZone};
use rune::{
    DisplayTarget, KeyPolicy, OverlayInput, OverlayOptions, OverlayPlacement, OverlaySpec,
    ScreenEdge,
};

use super::{
    Ledger, OptionPeekMode, Planner, option_peek_mode,
    peek::{self, Peek},
};
use crate::app_settings::AppSettings;
use crate::desktop::login_item::{LoginItemRequest, LoginItemStatus, LoginItemWorker};

static VISIBLE: AtomicBool = AtomicBool::new(false);
static OPEN: AtomicBool = AtomicBool::new(false);
static PEEK_PINNED: AtomicBool = AtomicBool::new(false);
static INTENTIONAL_DICTATION: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Encoded `OptionPeekMode`: 0 = TapOrHold, 1 = TapOnly, 2 = MenuBarOnly.
static OPTION_MODE: AtomicU8 = AtomicU8::new(0);

fn encode_option_mode(mode: OptionPeekMode) -> u8 {
    match mode {
        OptionPeekMode::TapOrHold => 0,
        OptionPeekMode::TapOnly => 1,
        OptionPeekMode::MenuBarOnly => 2,
    }
}

pub(crate) fn enabled_now() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// True when a clean Option tap should toggle Today's Steps (any mode except bare-Option Toggle dictation).
pub(crate) fn option_tap_enabled_now() -> bool {
    OPTION_MODE.load(Ordering::Acquire) != encode_option_mode(OptionPeekMode::MenuBarOnly)
}

/// True when Planner is enabled and dictation uses bare Option in Hold mode, so short
/// Option taps toggle Today's Steps silently while holds >= 300 ms dictate as usual.
pub(crate) fn bare_option_tap_only_now() -> bool {
    enabled_now()
        && OPTION_MODE.load(Ordering::Acquire) == encode_option_mode(OptionPeekMode::TapOnly)
}

pub(crate) fn peek_pinned_now() -> bool {
    PEEK_PINNED.load(Ordering::Acquire)
}

pub(crate) fn open() {
    PEEK_PINNED.store(false, Ordering::Release);
    OPEN.store(true, Ordering::Release);
    crate::macos_rune::wake_pump();
}

/// Toggles the pinned Today's Steps overlay from the macOS menu bar without holding Option.
pub(crate) fn toggle_peek() {
    let next = !PEEK_PINNED.load(Ordering::Acquire);
    PEEK_PINNED.store(next, Ordering::Release);
    crate::macos_rune::wake_pump();
}

/// Called only once a dictation capture becomes intentional (past the 300 ms
/// hold threshold or started explicitly via control/API) so provisional Option
/// taps can close a pinned overlay cleanly without flickering and reopening.
pub(crate) fn on_dictation_intentional() {
    INTENTIONAL_DICTATION.store(true, Ordering::Release);
    if PEEK_PINNED.swap(false, Ordering::AcqRel) {
        crate::macos_rune::wake_pump();
    }
}

/// Clears the active intentional-dictation latch as soon as audio capture ends
/// (even while transcription/rewriting finishes), so tapping Option right after
/// dictating opens Today's Steps instead of being dropped.
pub(crate) fn on_dictation_capture_ended() {
    if INTENTIONAL_DICTATION.swap(false, Ordering::AcqRel) {
        crate::macos_rune::wake_pump();
    }
}

/// Called from the recognition event tap on the release of a clean single Option
/// tap (< `OPTION_TAP_MAX`, no other modifier/key/click, and not locking dictation).
pub(crate) fn on_option_tap() {
    if VISIBLE.load(Ordering::Acquire) || !option_tap_enabled_now() {
        return;
    }
    toggle_peek();
}

pub(super) fn visible(value: bool) {
    VISIBLE.store(value, Ordering::Release);
}

pub(crate) struct Driver {
    panel: rune::OverlayHandle<Planner>,
    peek: rune::OverlayHandle<Peek>,
    hold_preview: peek::HoldPreviewGate,
    option_active: bool,
    dictating: bool,
    pinned: bool,
    last: Instant,
    enabled: bool,
    loaded: bool,
    login: Option<LoginItemWorker>,
}

pub(crate) fn install<S: 'static>(app: &mut rune::App<S>) -> Driver {
    let panel = app.overlay(
        OverlaySpec {
            name: "Hex Planner",
            size: (980., 760.),
            edge: ScreenEdge::Top,
            margin: 0.,
            align: rune::Align::Center,
            inset: 0.,
            interactive: true,
            view: super::view::view,
        }
        .options(OverlayOptions {
            placement: Some(OverlayPlacement::FullScreen),
            display: DisplayTarget::UnderPointer,
            keys: KeyPolicy::OnDemand,
            input: OverlayInput::All,
            ..Default::default()
        })
        // Above ordinary apps, below system modal/security UI and the menu bar.
        .macos_level(3)
        .on_key(super::on_key)
        .poll(super::poll)
        .release_gpu_when_hidden(),
        Planner::new(Ledger::default(), 0, None),
    );
    let own = panel.clone();
    panel.update(move |state, _| state.panel = Some(own));
    let peek = app.overlay(
        OverlaySpec {
            name: "Hex Planner tasks",
            size: peek::SIZE,
            edge: ScreenEdge::Top,
            margin: 44.,
            align: rune::Align::End,
            inset: 20.,
            interactive: true,
            view: peek::view,
        }
        .options(OverlayOptions {
            display: DisplayTarget::UnderPointer,
            keys: KeyPolicy::OnDemand,
            input: OverlayInput::Regions,
            ..Default::default()
        })
        .macos_level(3)
        .on_key(peek::on_key)
        .on_focus(peek::on_focus)
        .release_gpu_when_hidden(),
        Peek::default(),
    );
    let panel_for_toggle = panel.clone();
    let peek_handle = peek.clone();
    peek.update(move |state, _| {
        state.handle = Some(peek_handle);
        state.on_toggle = Some(Arc::new(move |index| {
            // Cancel any Option-triggered dictation so releasing Option after
            // checking a step never transcribes or pastes into the active app.
            crate::recognition::cancel_shortcut_dictation();
            panel_for_toggle.update(move |planner, _| {
                planner.toggle_today_step(index);
            });
            crate::macos_rune::wake_pump();
        }));
        state.on_open = Some(Arc::new(|| {
            crate::recognition::cancel_shortcut_dictation();
            open();
        }));
        state.on_pin = Some(Arc::new(|value| {
            if value {
                crate::recognition::cancel_shortcut_dictation();
            }
            PEEK_PINNED.store(value, Ordering::Release);
            crate::macos_rune::wake_pump();
        }));
    });
    Driver {
        panel,
        peek,
        hold_preview: peek::HoldPreviewGate::default(),
        option_active: false,
        dictating: false,
        pinned: false,
        last: Instant::now() - Duration::from_secs(1),
        enabled: false,
        loaded: false,
        login: None,
    }
}

impl Driver {
    pub(crate) fn tick(&mut self, settings: &AppSettings) {
        // Backdrops are ordinary floating panels, never event taps or a kiosk mode.
        // Re-enumerate while visible so newly attached displays are covered too.
        backdrops(VISIBLE.load(Ordering::Acquire));
        let feature = crate::labs::Feature::Planner;
        let enabled = feature.shipped() && feature.on(settings) && feature.managed().is_none();
        ENABLED.store(enabled, Ordering::Release);
        let option_mode = option_peek_mode(settings);
        OPTION_MODE.store(encode_option_mode(option_mode), Ordering::Release);
        let engine_busy = crate::transcription::engine::dictation_busy();
        let mut pin_state = peek::PeekPinState {
            pinned: PEEK_PINNED.load(Ordering::Acquire),
            intentional_dictation: INTENTIONAL_DICTATION.load(Ordering::Acquire),
        };
        let dictating = enabled && pin_state.on_tick(engine_busy);
        INTENTIONAL_DICTATION.store(pin_state.intentional_dictation, Ordering::Release);
        if dictating {
            PEEK_PINNED.store(false, Ordering::Release);
        }
        // Show Today's Steps while Option is held alone, also when bare Option
        // dictates in Hold mode (the list stays up beside the recording, and a
        // click on it cancels that dictation). Wait `OPTION_HOLD_DELAY`
        // (== `OPTION_TAP_MAX`, 280 ms) so taps never cross the preview threshold.
        let option_raw = enabled
            && option_mode != OptionPeekMode::MenuBarOnly
            && crate::suppression::option_alone_held_now();
        let now_instant = Instant::now();
        let prev_option = self.option_active;
        self.option_active = self.hold_preview.tick(option_raw, now_instant);
        let option = self.option_active;
        let pinned = enabled && !dictating && PEEK_PINNED.load(Ordering::Acquire);
        let peek_trigger_changed =
            option != prev_option || dictating != self.dictating || pinned != self.pinned;
        self.dictating = dictating;
        self.pinned = pinned;
        let manual = OPEN.swap(false, Ordering::AcqRel);
        if !enabled {
            PEEK_PINNED.store(false, Ordering::Release);
            if self.enabled {
                Peek::present(&self.peek, Peek::default());
                self.panel.update(|state, _| {
                    state.save();
                    state.visible = false;
                    visible(false);
                });
                self.panel.hide();
            }
            self.enabled = false;
            return;
        }
        if !self.enabled {
            self.enabled = true;
            self.login = LoginItemWorker::new().ok();
            if let Some(login) = &mut self.login {
                if let Err(error) = login.request(LoginItemRequest::SetEnabled(true)) {
                    self.panel
                        .update(move |state, _| state.login_note = Some(error));
                }
            } else {
                self.panel.update(|state, _| {
                    state.login_note =
                        Some("Could not enable Launch at login. Open General to try again.".into())
                });
            }
        }
        if let Some(login) = &mut self.login {
            let result = login.poll();
            if !matches!(result, Ok(None)) {
                let note = match result {
                    Ok(Some(Ok(LoginItemStatus::Enabled))) => None,
                    Ok(Some(Ok(LoginItemStatus::RequiresApproval))) => Some("Allow Hex in System Settings → General → Login Items so your planner starts at login.".into()),
                    Ok(Some(Ok(LoginItemStatus::Disabled))) => Some("Launch at login is off. Open General to enable it.".into()),
                    Ok(Some(Err(error))) => Some(format!("Launch at login: {}", error.message)),
                    Err(error) => Some(format!("Launch at login: {error}")),
                    Ok(None) => unreachable!(),
                };
                self.panel.update(move |state, _| state.login_note = note);
            }
        }
        // While Option is held or state changed, refresh promptly without waiting the 1s cadence.
        if !manual
            && !peek_trigger_changed
            && !option_raw
            && self.last.elapsed() < Duration::from_secs(1)
        {
            return;
        }
        self.last = now_instant;
        let now = crate::history::now_ms() / 1_000;
        let date = day_at(now);
        let away = crate::audio::system_idle::released();
        if away {
            PEEK_PINNED.store(false, Ordering::Release);
        }
        let load = !self.loaded;
        self.loaded = true;
        let peek = self.peek.clone();
        self.panel.update(move |state, tx| {
            if load {
                let panel = state.panel.take();
                let note = state.login_note.take();
                let path = crate::app_paths::support_dir().map(|root| root.join("planner.json"));
                let loaded = path.as_ref().map_err(|error| error.to_string())
                    .and_then(|path| Ledger::load(path).map_err(|error| error.to_string()));
                match loaded {
                    Ok(ledger) => *state = Planner::new(ledger, date, path.ok()),
                    Err(error) => {
                        *state = Planner::new(Ledger::default(), date, None);
                        state.unreadable = true;
                        state.error = Some(format!("Could not read planner.json: {error}. The file was left untouched. Fix it and restart Hex; you can close anytime."));
                    }
                }
                state.panel = panel;
                state.login_note = note;
            }
            state.option_mode = option_mode;
            if date != state.date {
                state.save();
                state.date = date;
                state.restore();
            }
            if away {
                if state.visible {
                    state.save();
                    state.visible = false;
                    visible(false);
                    if let Some(panel) = &state.panel {
                        panel.hide();
                    }
                }
            } else if manual || (!state.visible && state.ledger.due(date, now)) {
                PEEK_PINNED.store(false, Ordering::Release);
                state.reveal(manual);
                tx.focus("planner-content", false);
            }
            let pinned_now = PEEK_PINNED.load(Ordering::Acquire);
            Peek::present(
                &peek,
                // Holding Option keeps the list up even while that hold dictates.
                Peek::snapshot(state, option, away, dictating && !option, pinned_now),
            );
        });
    }
}

pub(super) fn day_at(now: u64) -> i64 {
    let zone = NSTimeZone::timeZoneWithName(&NSString::from_str("America/New_York"))
        .expect("macOS provides the America/New_York time zone");
    let date = NSDate::dateWithTimeIntervalSince1970(now as f64);
    super::model::planning_day(now as i64, zone.secondsFromGMTForDate(&date) as i64)
}

thread_local! {
    static BACKDROPS: RefCell<Vec<(u32, Retained<NSPanel>)>> = const { RefCell::new(Vec::new()) };
}

fn backdrops(shown: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    BACKDROPS.with(|slot| {
        let mut panels = slot.borrow_mut();
        if !shown {
            for (_, panel) in panels.iter() {
                panel.orderOut(None);
            }
            panels.clear();
            return;
        }
        let screens = NSScreen::screens(mtm);
        let mut present = Vec::new();
        for screen in screens {
            let number = screen
                .deviceDescription()
                .objectForKey(&NSString::from_str("NSScreenNumber"));
            let Some(id) = number.and_then(|number| {
                number
                    .downcast_ref::<NSNumber>()
                    .map(NSNumber::unsignedIntValue)
            }) else {
                continue;
            };
            present.push(id);
            if let Some((_, panel)) = panels.iter().find(|(number, _)| *number == id) {
                panel.setFrame_display(screen.frame(), true);
                continue;
            }
            // Native panels are created and retained only on AppKit's main thread.
            let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
                NSPanel::alloc(mtm),
                screen.frame(),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            );
            unsafe {
                panel.setReleasedWhenClosed(false);
            }
            panel.setLevel(2);
            panel.setHidesOnDeactivate(false);
            panel.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
            panel.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
            panel.orderFrontRegardless();
            panels.push((id, panel));
        }
        panels.retain(|(id, panel)| {
            let keep = present.contains(id);
            if !keep {
                panel.orderOut(None);
            }
            keep
        });
    });
}
