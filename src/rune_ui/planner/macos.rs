//! Native presentation and the timer that also runs with Settings closed.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
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

use super::{Ledger, Planner};
use crate::app_settings::AppSettings;
use crate::desktop::login_item::{LoginItemRequest, LoginItemStatus, LoginItemWorker};

static VISIBLE: AtomicBool = AtomicBool::new(false);
static OPEN: AtomicBool = AtomicBool::new(false);

pub(crate) fn open() {
    OPEN.store(true, Ordering::Release);
    crate::macos_rune::wake_pump();
}

pub(super) fn visible(value: bool) {
    VISIBLE.store(value, Ordering::Release);
}

pub(crate) struct Driver {
    panel: rune::OverlayHandle<Planner>,
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
    Driver {
        panel,
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
        let manual = OPEN.swap(false, Ordering::AcqRel);
        if !enabled {
            if self.enabled {
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
        if !manual && self.last.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last = Instant::now();
        let now = crate::history::now_ms() / 1_000;
        let date = day_at(now);
        let away = crate::audio::system_idle::released();
        let load = !self.loaded;
        self.loaded = true;
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
                        state.error = Some(format!("Could not read planner.json: {error}. The file was left untouched. Fix it and restart Hex; emergency bypass remains available."));
                    }
                }
                state.panel = panel;
                state.login_note = note;
            }
            if date != state.date {
                state.save();
                state.date = date;
                state.restore();
            }
            if away {
                if state.visible {
                    state.save(); state.visible = false; visible(false);
                    if let Some(panel) = &state.panel { panel.hide(); }
                }
            } else if manual || (!state.visible && state.ledger.due(date, now)) {
                state.reveal(manual);
                tx.focus("planner-content", false);
            }
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
