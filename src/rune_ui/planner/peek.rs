//! Interactive floating overlay of today's steps, toggled with an Option tap (or held while Option is down when dictation uses another key).

use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

use rune::{Cursor, Cx, El, Key, KeyEvent, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant};
use rune_theme_opencode::icons;

use super::{OptionPeekMode, Paint, Planner, model};

pub(super) const SIZE: (f32, f32) = (368., 540.);

/// Maximum duration of a clean Option-alone tap to toggle Today's Steps without
/// colliding with Hold-mode dictation (`MINIMUM_HOLD_DURATION`).
#[cfg(target_os = "macos")]
pub(crate) const OPTION_TAP_MAX: Duration =
    crate::dictation::MINIMUM_HOLD_DURATION.saturating_sub(Duration::from_millis(20));

/// Minimum time Option must be held alone before showing the temporary hold preview
/// (when dictation doesn't use bare Option). Equal to `OPTION_TAP_MAX` so no short
/// tap ever flashes the preview and no preview ever collides with tap-to-pin.
#[cfg(target_os = "macos")]
pub(crate) const OPTION_HOLD_DELAY: Duration = OPTION_TAP_MAX;

/// Deferral window when bare-Option double-tap dictation lock is enabled, so a
/// double-tap-to-lock gesture never flashes Today's Steps between tap 1 and tap 2.
#[cfg(target_os = "macos")]
pub(crate) const DOUBLE_TAP_DEFER: Duration = crate::dictation::DOUBLE_TAP_WINDOW;

/// Pure pin/dictation state shared by the macOS driver and recognition tests:
/// provisional shortcut presses (< 300 ms) must NEVER unpin Today's Steps, so
/// tapping Option while pinned closes it cleanly instead of flickering and
/// reopening on release. Only an intentional dictation (>= 300 ms or control)
/// unpins and hides the overlay.
#[cfg(target_os = "macos")]
#[derive(Default)]
pub(super) struct PeekPinState {
    pub pinned: bool,
    pub intentional_dictation: bool,
}

#[cfg(target_os = "macos")]
impl PeekPinState {
    #[cfg(test)]
    pub(super) fn on_dictation_intentional(&mut self) {
        self.intentional_dictation = true;
        self.pinned = false;
    }

    pub(super) fn on_tick(&mut self, engine_busy: bool) -> bool {
        if !engine_busy {
            self.intentional_dictation = false;
        }
        let dictating = engine_busy && self.intentional_dictation;
        if dictating {
            self.pinned = false;
        }
        dictating
    }

    #[cfg(test)]
    pub(super) fn on_option_tap(&mut self) {
        self.pinned = !self.pinned;
    }
}

/// Pure timer state machine used by the macOS driver to decide whether a
/// temporary Option-hold preview should be shown. Activates only after Option
/// has been held alone continuously for [`OPTION_HOLD_DELAY`].
#[cfg(target_os = "macos")]
#[derive(Default)]
pub(super) struct HoldPreviewGate {
    since: Option<Instant>,
    active: bool,
}

#[cfg(target_os = "macos")]
impl HoldPreviewGate {
    pub(super) fn tick(&mut self, held_alone: bool, now: Instant) -> bool {
        if held_alone {
            let started = *self.since.get_or_insert(now);
            self.active = now.duration_since(started) >= OPTION_HOLD_DELAY;
        } else {
            self.since = None;
            self.active = false;
        }
        self.active
    }
}

/// Pure state machine observing raw input edges to detect a clean single tap of
/// Option alone (< 280 ms, no other modifier, key, or mouse click), optionally
/// waiting out the double-tap window so double-tap dictation lock never flashes.
#[cfg(target_os = "macos")]
#[derive(Default)]
pub(crate) struct OptionTapDetector {
    pressed_at: Option<crate::audio::CaptureInstant>,
    contaminated: bool,
    pending_tap_at: Option<crate::audio::CaptureInstant>,
}

#[cfg(target_os = "macos")]
impl OptionTapDetector {
    /// Observes one modifier/key/mouse event. Returns `true` immediately on a
    /// clean Option-alone tap when `defer_for_double_tap` is false; when true,
    /// arms `pending_tap_at` for [`Self::poll_deferred`] and cancels it if any
    /// second Option press or other input arrives within `DOUBLE_TAP_DEFER`.
    pub(crate) fn observe(
        &mut self,
        event: crate::suppression::InputEvent,
        now: crate::audio::CaptureInstant,
        defer_for_double_tap: bool,
    ) -> bool {
        use crate::app_settings::{
            COMMAND_KEY_MASK, CONTROL_KEY_MASK, OPTION_KEY_MASK, SHIFT_KEY_MASK,
        };
        use crate::suppression::InputEvent;

        const RELEVANT_MODS: u64 =
            OPTION_KEY_MASK | SHIFT_KEY_MASK | COMMAND_KEY_MASK | CONTROL_KEY_MASK;

        match event {
            InputEvent::Flags(flags) => {
                let mods = flags & RELEVANT_MODS;
                if mods == OPTION_KEY_MASK {
                    // A second Option press during the double-tap window cancels any
                    // pending single-tap toggle (the user is double-tapping to lock dictation).
                    if self.pending_tap_at.is_some() {
                        self.pending_tap_at = None;
                        self.contaminated = true;
                        self.pressed_at = None;
                        return false;
                    }
                    if self.pressed_at.is_none() && !self.contaminated {
                        self.pressed_at = Some(now);
                    }
                    false
                } else if mods == 0 {
                    let tapped = self.pressed_at.take().is_some_and(|started| {
                        !self.contaminated && now.duration_since(started) < OPTION_TAP_MAX
                    });
                    self.contaminated = false;
                    if tapped {
                        if defer_for_double_tap {
                            self.pending_tap_at = Some(now);
                            false
                        } else {
                            true
                        }
                    } else {
                        false
                    }
                } else {
                    self.pending_tap_at = None;
                    self.contaminated = true;
                    self.pressed_at = None;
                    false
                }
            }
            _ => {
                self.pending_tap_at = None;
                self.contaminated = true;
                self.pressed_at = None;
                false
            }
        }
    }

    /// Fires a deferred single-tap once `DOUBLE_TAP_DEFER` has elapsed with no
    /// second Option press and no active dictation recording.
    pub(crate) fn poll_deferred(
        &mut self,
        now: crate::audio::CaptureInstant,
        recording: bool,
    ) -> bool {
        if recording {
            self.pending_tap_at = None;
            return false;
        }
        if self
            .pending_tap_at
            .is_some_and(|at| now.duration_since(at) >= DOUBLE_TAP_DEFER)
        {
            self.pending_tap_at = None;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PeekStep {
    pub index: usize,
    pub text: String,
    pub done: bool,
}

#[derive(Default)]
pub(super) struct Peek {
    pub shown: bool,
    pub pinned: bool,
    pub committed: bool,
    pub option_mode: OptionPeekMode,
    pub date_label: String,
    pub focus: String,
    pub steps: Vec<PeekStep>,
    /// Highlighted row in display order when pinned and navigated with ↑/↓.
    pub selected: Option<usize>,
    pub on_toggle: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    pub on_open: Option<Arc<dyn Fn() + Send + Sync>>,
    pub on_pin: Option<Arc<dyn Fn(bool) + Send + Sync>>,
    #[cfg(target_os = "macos")]
    pub handle: Option<rune::OverlayHandle<Self>>,
}

impl Peek {
    pub(super) fn snapshot(
        planner: &Planner,
        option: bool,
        away: bool,
        dictating: bool,
        pinned: bool,
    ) -> Self {
        let shown =
            (option || pinned) && !away && !dictating && !planner.visible && !planner.unreadable;
        if !shown {
            return Self::default();
        }
        let day = planner.ledger.day_ref(planner.date);
        let committed = day.is_some_and(|day| day.committed);
        let focus = day
            .map(|day| day.problem.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        let steps = day
            .into_iter()
            .flat_map(|day| day.steps.iter().enumerate())
            .filter(|(_, step)| !step.text.trim().is_empty())
            .map(|(index, step)| PeekStep {
                index,
                text: step.text.clone(),
                done: step.done,
            })
            .collect();
        Self {
            shown,
            pinned,
            committed,
            option_mode: planner.option_mode,
            date_label: model::human_date_label(planner.date),
            focus,
            steps,
            selected: None,
            on_toggle: None,
            on_open: None,
            on_pin: None,
            #[cfg(target_os = "macos")]
            handle: None,
        }
    }

    pub(super) fn same_data(&self, other: &Self) -> bool {
        self.shown == other.shown
            && self.pinned == other.pinned
            && self.committed == other.committed
            && self.option_mode == other.option_mode
            && self.date_label == other.date_label
            && self.focus == other.focus
            && self.steps == other.steps
    }

    /// Display order matches the ledger order so steps never jump under the pointer or highlight.
    fn ordered_indices(&self) -> Vec<usize> {
        self.steps.iter().map(|s| s.index).collect()
    }

    pub(super) fn toggle_step(&mut self, index: usize) {
        if !self.committed {
            return;
        }
        if let Some(pos) = self.steps.iter().position(|step| step.index == index) {
            self.steps[pos].done = !self.steps[pos].done;
            if self.pinned {
                self.selected = Some(pos);
                #[cfg(target_os = "macos")]
                if let Some(handle) = &self.handle {
                    handle.request_keys();
                }
            }
        }
        if let Some(callback) = &self.on_toggle {
            callback(index);
        }
    }

    pub(super) fn set_pinned(&mut self, pinned: bool) {
        self.pinned = pinned;
        if !pinned {
            self.selected = None;
        }
        if let Some(callback) = &self.on_pin {
            callback(pinned);
        }
    }

    pub(super) fn open_planner(&self) {
        if let Some(callback) = &self.on_open {
            callback();
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn present(handle: &rune::OverlayHandle<Self>, next: Self) {
        let own = handle.clone();
        handle.update_if(move |state, tx| {
            if state.same_data(&next) {
                return false;
            }
            if state.shown != next.shown {
                if next.shown {
                    own.show();
                } else {
                    own.hide();
                }
            }
            state.shown = next.shown;
            state.pinned = next.pinned;
            state.committed = next.committed;
            state.option_mode = next.option_mode;
            state.date_label = next.date_label;
            state.focus = next.focus;
            state.steps = next.steps;
            if !next.pinned || state.selected.is_some_and(|i| i >= state.steps.len()) {
                state.selected = None;
            }
            // Never request_keys() on open: glancing at Today's Steps must not swallow
            // typing in the active app. Keys are only taken after clicking inside a pinned card.
            tx.instant();
            true
        });
    }
}

pub(super) fn on_focus(state: &mut Peek, focused: bool, _: &mut Tx) {
    if !focused {
        state.selected = None;
    }
}

pub(super) fn on_key(state: &mut Peek, event: &KeyEvent, tx: &mut Tx) -> bool {
    if !state.pinned || tx.key_repeat() {
        return false;
    }
    let order = state.ordered_indices();
    match event.key {
        Key::Escape => {
            state.set_pinned(false);
            state.shown = false;
            true
        }
        Key::Down if !order.is_empty() => {
            tx.blur();
            let next = match state.selected {
                None => 0,
                Some(i) => (i + 1) % order.len(),
            };
            state.selected = Some(next);
            true
        }
        Key::Up if !order.is_empty() => {
            tx.blur();
            let next = match state.selected {
                None => order.len() - 1,
                Some(0) => order.len() - 1,
                Some(i) => i - 1,
            };
            state.selected = Some(next);
            true
        }
        Key::Text(ref t) if t == " " && state.committed && !order.is_empty() => {
            let pos = state.selected.unwrap_or(0).min(order.len() - 1);
            state.toggle_step(order[pos]);
            true
        }
        Key::Enter if state.committed && !order.is_empty() => {
            let pos = state.selected.unwrap_or(0).min(order.len() - 1);
            state.toggle_step(order[pos]);
            true
        }
        _ => false,
    }
}

fn checkbox_box(done: bool, p: Paint) -> El {
    let mut box_el = el()
        .key("checkbox-box")
        .size(18., 18.)
        .mt(1.)
        .center()
        .radius(5.)
        .a11y_hidden();
    if done {
        let (fill, check_color) = if p.k.dark {
            (p.k.c.success.fg, p.k.c.bg_base)
        } else {
            (p.k.c.success.icon, p.k.c.white)
        };
        box_el = box_el
            .fill(fill)
            .border(1., fill)
            .child(icon(&icons::CHECK, 12.).color(check_color));
    } else {
        box_el = box_el.fill(p.k.c.bg_base).border(1.5, p.k.c.border_strong);
    }
    box_el
}

fn step_row(step: &PeekStep, checkable: bool, highlighted: bool, p: Paint) -> El {
    let step_index = step.index;
    let mut label = text(&step.text)
        .key("task")
        .font(p.k.ty.ui)
        .color(if step.done { p.muted() } else { p.text() })
        .wrap_anywhere()
        .grow(1.)
        .shrink();
    if step.done {
        label = label.strikes(super::view::word_strikes(&step.text, p.muted()));
    }
    let border_color = if highlighted {
        p.k.c.border_focus
    } else if step.done {
        p.k.c.border_muted
    } else {
        p.k.c.border_base
    };
    let mut inner = row()
        .key(format!("todo-{step_index}"))
        .w_full()
        .gap(12.)
        .px(12.)
        .py(10.)
        .radius(p.k.radius.md)
        .fill(if step.done {
            p.k.c.bg_base
        } else {
            p.k.c.layer_01
        })
        .border(if highlighted { 1.5 } else { p.hair }, border_color);

    if checkable {
        inner = inner
            .role("checkbox")
            .label(&step.text)
            .checked(step.done)
            .cursor(Cursor::Pointer)
            .hit()
            .on_press(move |state: &mut Peek| state.toggle_step(step_index))
            .child(checkbox_box(step.done, p))
            .child(label);
        p.k.interactive(inner)
    } else {
        inner
            .child(
                el().size(6., 6.)
                    .mt(7.)
                    .radius(999.)
                    .fill(p.k.c.border_strong)
                    .a11y_hidden(),
            )
            .child(label)
    }
}

pub(super) fn view(state: &Peek, cx: &mut Cx<Peek>) -> El {
    let p = Paint::new(cx.system_dark(), cx.env().scale);
    let total = state.steps.len();
    let done = state.steps.iter().filter(|step| step.done).count();
    let all_done = state.committed && total > 0 && done == total;
    let is_draft = !state.committed && total > 0;

    let has_body_open_cta = is_draft || total == 0;
    let open_btn = Button::new("Open planner")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut Peek| s.open_planner())
        .build(p.k)
        .key("peek-open")
        .hit();

    let mut top_row = row()
        .w_full()
        .center_y()
        .gap(6.)
        .child(text("Today").font(p.k.ty.ui_medium).color(p.text()))
        .child(
            text(format!("· {}", state.date_label))
                .font(p.k.ty.small)
                .color(p.muted())
                .grow(1.)
                .shrink()
                .truncate(),
        );

    if state.pinned {
        if !has_body_open_cta {
            top_row = top_row.child(open_btn);
        }
        top_row = top_row.child(
            IconButton::new(&icons::CLOSE)
                .label("Close today’s steps")
                .variant(IconButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut Peek| {
                    s.set_pinned(false);
                    s.shown = false;
                })
                .build(p.k)
                .key("peek-close")
                .hit(),
        );
    } else {
        top_row = top_row.child(
            Button::new("Pin")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press_tx(|s: &mut Peek, tx| {
                    s.set_pinned(true);
                    tx.blur();
                })
                .build(p.k)
                .label("Keep open after releasing Option")
                .key("peek-pin")
                .hit(),
        );
        if !has_body_open_cta {
            top_row = top_row.child(open_btn);
        }
    }

    let mut header = col()
        .key("peek-header")
        .w_full()
        .px(16.)
        .pt(14.)
        .pb(12.)
        .gap(8.)
        .child(top_row);

    if !state.focus.is_empty() {
        header = header.child(
            text(&state.focus)
                .key("peek-focus")
                .font(p.k.ty.ui)
                .color(p.text())
                .wrap_anywhere(),
        );
    }

    if is_draft {
        header = header.child(
            row()
                .key("peek-draft-banner")
                .w_full()
                .center_y()
                .gap(8.)
                .child(
                    row()
                        .px(8.)
                        .h(20.)
                        .center()
                        .radius(999.)
                        .fill(p.k.c.warning.bg)
                        .border(p.hair, p.k.c.warning.border)
                        .child(
                            text("Draft · not committed")
                                .font(p.k.ty.small_medium)
                                .color(super::view::warning_text(p)),
                        ),
                )
                .child(el().grow(1.))
                .child(
                    Button::new("Finish planning →")
                        .variant(ButtonVariant::Submit)
                        .control_size(ControlSize::Small)
                        .on_press(|s: &mut Peek| s.open_planner())
                        .build(p.k)
                        .key("peek-finish-draft")
                        .hit(),
                ),
        );
    } else if total > 0 {
        let ratio = (done as f32 / total as f32).clamp(0.0, 1.0);
        header = header.child(
            row()
                .key("peek-progress")
                .w_full()
                .center_y()
                .gap(10.)
                .child(
                    row()
                        .grow(1.)
                        .h(4.)
                        .radius(999.)
                        .fill(p.k.c.layer_02)
                        .clip_soft(0.)
                        .child(el().h_full().w_frac(ratio).radius(999.).fill(if all_done {
                            p.k.c.success.fg
                        } else {
                            p.text()
                        })),
                )
                .child(
                    text(if all_done {
                        "All done".into()
                    } else {
                        format!("{done} of {total}")
                    })
                    .key("peek-count")
                    .font(p.k.ty.small_medium.features("'tnum' 1"))
                    .color(if all_done { p.success() } else { p.muted() }),
                ),
        );
    }

    let mut cards = col().key("todo-cards").w_full().gap(8.).px(12.).py(12.);

    if all_done {
        cards = cards.child(
            row()
                .key("all-done-banner")
                .w_full()
                .px(12.)
                .py(10.)
                .gap(8.)
                .center_y()
                .radius(p.k.radius.md)
                .fill(p.k.c.success.bg)
                .border(p.hair, p.k.c.success.border)
                .child(icon(&icons::CHECK, 14.).color(p.success()))
                .child(
                    text("All steps complete for today.")
                        .font(p.k.ty.ui_medium)
                        .color(p.success()),
                ),
        );
    }

    for (display_pos, step) in state.steps.iter().enumerate() {
        let hi = state.pinned && state.selected == Some(display_pos);
        cards = cards.child(step_row(step, state.committed, hi, p));
    }

    if state.steps.is_empty() {
        cards = cards.child(
            col()
                .key("no-todos")
                .w_full()
                .padding(16.)
                .gap(10.)
                .radius(p.k.radius.md)
                .fill(p.k.c.layer_01)
                .border(p.hair, p.k.c.border_base)
                .child(
                    text("No plan for today yet")
                        .font(p.k.ty.ui_medium)
                        .color(p.text()),
                )
                .child(
                    text("Take a few minutes to pick your focus and commit to today’s steps.")
                        .font(p.k.ty.small)
                        .color(p.muted())
                        .wrap(),
                )
                .child(
                    Button::new("Plan today")
                        .variant(ButtonVariant::Submit)
                        .control_size(ControlSize::Small)
                        .on_press(|s: &mut Peek| s.open_planner())
                        .build(p.k)
                        .key("peek-plan-today")
                        .hit(),
                ),
        );
    }

    let footer_hint = if total == 0 {
        if state.pinned {
            match state.option_mode {
                OptionPeekMode::MenuBarOnly => "Menu bar or × closes",
                _ => "⌥ or × closes",
            }
        } else {
            "Release ⌥ to close"
        }
    } else if is_draft {
        if state.pinned {
            match state.option_mode {
                OptionPeekMode::MenuBarOnly => "Commit to check off · × closes",
                _ => "Commit to check off · ⌥ or × closes",
            }
        } else {
            "Commit your plan to check off steps"
        }
    } else if state.pinned && state.selected.is_some() {
        match state.option_mode {
            OptionPeekMode::MenuBarOnly => "↑↓ + Space to check off · Esc or × closes",
            _ => "↑↓ + Space to check off · ⌥, Esc, or × closes",
        }
    } else if state.pinned {
        match state.option_mode {
            OptionPeekMode::MenuBarOnly => "Click a step to check off · × closes",
            _ => "Click a step to check off · ⌥ or × closes",
        }
    } else {
        "Click any step to check off · Pin keeps open"
    };

    let surface = col()
        .key("peek-surface")
        .w_full()
        .max_h(SIZE.1 - 16.)
        .radius(p.k.radius.xl)
        .fill(p.k.c.bg_base)
        .border(p.hair, p.k.c.border_base)
        .clip_rounded(p.k.radius.xl)
        .hit()
        .child(header)
        .child(el().w_full().h(p.hair).fill(p.k.c.border_muted))
        .child(
            col()
                .key("todo-scroll")
                .w_full()
                .max_h(SIZE.1 - 168.)
                .shrink()
                .hit()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(cards),
        )
        .child(el().w_full().h(p.hair).fill(p.k.c.border_muted))
        .child(
            row()
                .key("peek-footer")
                .w_full()
                .px(16.)
                .py(9.)
                .center_y()
                .child(
                    text(footer_hint)
                        .font(p.k.ty.small)
                        .color(p.muted())
                        .truncate(),
                ),
        );

    col()
        .key("planner-todos")
        .role("region")
        .label("Today's tasks")
        .w_full()
        .h_full()
        .padding(8.)
        .child(p.k.elevate(surface, &p.k.elevation.floating))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::super::{Ledger, Step, model::Day};
    use super::*;
    use rune::{Env, Mods, Runtime, text::TextSystem};

    fn runtime_with(state: Peek, dark: bool) -> Runtime<Peek> {
        let text = TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
        let mut rt = super::super::super::offscreen::runtime(
            state,
            view,
            SIZE,
            Env {
                scale: 1.,
                system_dark: dark,
            },
            text,
        );
        rt.on_key = Some(on_key);
        rt.frame(0.);
        rt
    }

    fn runtime(steps: Vec<PeekStep>, dark: bool) -> Runtime<Peek> {
        runtime_with(
            Peek {
                shown: true,
                pinned: false,
                committed: !steps.is_empty(),
                option_mode: OptionPeekMode::TapOrHold,
                date_label: "Fri, Nov 20".into(),
                focus: "Make the daily planning overlay feel effortless".into(),
                steps,
                selected: None,
                on_toggle: None,
                on_open: None,
                on_pin: None,
                #[cfg(target_os = "macos")]
                handle: None,
            },
            dark,
        )
    }

    #[test]
    fn option_peek_reads_todays_steps_and_respects_visibility_rules() {
        let mut ledger = Ledger::default();
        for date in [99, 100] {
            ledger.days.push(Day {
                date,
                problem: format!("Focus {date}"),
                committed: true,
                steps: vec![
                    Step {
                        text: format!("Unfinished {date}"),
                        done: false,
                    },
                    Step {
                        text: "Already finished".into(),
                        done: true,
                    },
                ],
                ..Default::default()
            });
        }
        let mut planner = Planner::new(ledger, 100, None);
        let peek = Peek::snapshot(&planner, true, false, false, false);
        assert!(peek.shown);
        assert!(peek.committed);
        assert_eq!(peek.steps.len(), 2);
        assert_eq!(peek.steps[0].text, "Unfinished 100");
        assert!(!peek.steps[0].done);
        assert!(peek.steps[1].done);
        assert!(
            !Peek::snapshot(&planner, false, false, false, false).shown,
            "release hides when not pinned"
        );
        assert!(
            Peek::snapshot(&planner, false, false, false, true).shown,
            "pinned stays visible after releasing Option"
        );
        assert!(
            !Peek::snapshot(&planner, true, false, true, false).shown,
            "active dictation suppresses when dictation is not bare Option"
        );
        assert!(
            !Peek::snapshot(&planner, true, true, false, true).shown,
            "lock/sleep hides even when pinned"
        );
        planner.visible = true;
        assert!(
            !Peek::snapshot(&planner, true, false, false, true).shown,
            "full planner takes priority"
        );
        planner.visible = false;
        planner.unreadable = true;
        assert!(!Peek::snapshot(&planner, true, false, false, false).shown);
        planner.unreadable = false;
        planner.date = 101;
        assert!(
            Peek::snapshot(&planner, true, false, false, false)
                .steps
                .is_empty(),
            "never show yesterday as today"
        );
    }

    #[test]
    fn option_peek_steps_are_checkable_pinnable_and_keyboard_controllable_when_pinned() {
        let toggled = Arc::new(AtomicUsize::new(usize::MAX));
        let opened = Arc::new(AtomicBool::new(false));
        let pinned = Arc::new(AtomicBool::new(false));
        let t = Arc::clone(&toggled);
        let o = Arc::clone(&opened);
        let p = Arc::clone(&pinned);
        let mut rt = runtime(
            vec![
                PeekStep {
                    index: 0,
                    text: "Ship checkable Option tasks".into(),
                    done: false,
                },
                PeekStep {
                    index: 1,
                    text: "Verify dark and light renders".into(),
                    done: true,
                },
            ],
            true,
        );
        rt.state.on_toggle = Some(Arc::new(move |idx| {
            t.store(idx, Ordering::Release);
        }));
        rt.state.on_open = Some(Arc::new(move || {
            o.store(true, Ordering::Release);
        }));
        rt.state.on_pin = Some(Arc::new(move |val| {
            p.store(val, Ordering::Release);
        }));

        let (x, y) = rt.find("todo-0").expect("todo-0 checkbox row");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.01);
        assert!(rt.state.steps[0].done, "clicking step checks it off");
        assert_eq!(toggled.load(Ordering::Acquire), 0);
        assert!(rt.find("all-done-banner").is_some());

        let (px, py) = rt.find("peek-pin").expect("pin button");
        rt.pointer_down(px, py);
        rt.pointer_up(px, py);
        rt.frame(0.01);
        assert!(rt.state.pinned);
        assert!(pinned.load(Ordering::Acquire));
        assert_eq!(
            rt.state.selected, None,
            "opening pinned does not steal keyboard focus until clicked or navigated"
        );
        assert!(
            rt.find("peek-close").is_some(),
            "pinned overlay shows close button"
        );

        // Down highlights step 0, then Space toggles it in place (pressing again undoes it cleanly).
        rt.key(KeyEvent {
            key: Key::Down,
            mods: Mods::default(),
        });
        rt.frame(0.01);
        assert_eq!(rt.state.selected, Some(0));
        rt.key(KeyEvent {
            key: Key::Text(" ".into()),
            mods: Mods::default(),
        });
        rt.frame(0.01);
        assert!(
            !rt.state.steps[0].done,
            "Space toggles highlighted step in place"
        );
        assert_eq!(
            rt.state.selected,
            Some(0),
            "highlight stays on the same step"
        );
        rt.key(KeyEvent {
            key: Key::Text(" ".into()),
            mods: Mods::default(),
        });
        rt.frame(0.01);
        assert!(rt.state.steps[0].done, "second Space undoes the same step");

        // Losing window focus clears keyboard selection so footer reverts from "↑↓ + Space".
        on_focus(&mut rt.state, false, &mut Tx::default());
        assert_eq!(rt.state.selected, None);

        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        rt.frame(0.01);
        assert!(!rt.state.pinned, "Escape unpins and closes pinned overlay");
        assert!(!rt.state.shown);

        let (ox, oy) = rt.find("peek-open").expect("open planner button");
        rt.pointer_down(ox, oy);
        rt.pointer_up(ox, oy);
        rt.frame(0.01);
        assert!(opened.load(Ordering::Acquire));
    }

    #[test]
    fn option_peek_marks_uncommitted_drafts_as_read_only() {
        let mut rt = runtime(
            vec![PeekStep {
                index: 0,
                text: "Uncommitted draft step".into(),
                done: false,
            }],
            true,
        );
        rt.state.committed = false;
        rt.invalidate();
        rt.frame(0.01);
        assert!(rt.find("peek-draft-banner").is_some());
        let (x, y) = rt.find("todo-0").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.01);
        assert!(
            !rt.state.steps[0].done,
            "draft steps are read-only until committed"
        );
    }

    #[test]
    fn option_peek_wraps_and_scrolls_every_task() {
        let steps: Vec<_> = (0..100)
            .map(|i| PeekStep {
                index: i,
                text: format!("Task {i}: {}", "a long step that wraps ".repeat(5)),
                done: false,
            })
            .collect();
        let mut rt = runtime(steps, true);
        let elements = rt.elements();
        assert_eq!(
            elements
                .iter()
                .filter(|e| e.key.as_deref() == Some("task"))
                .count(),
            100
        );
        let task = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("task"))
            .unwrap();
        assert!(task.rect[3] > 40. && task.rect[0] + task.rect[2] <= SIZE.0 - 12.);
        let scroll = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("todo-scroll"))
            .unwrap()
            .rect;
        assert!(scroll[3] <= SIZE.1 - 168.);
        assert!(
            !rt.hit_regions().is_empty(),
            "cards and scroll receive the pointer without keyboard focus"
        );
        rt.wheel(scroll[0] + 30., scroll[1] + 30., -100_000.);
        rt.settle_all();
        rt.frame(1.);
        let last = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("todo-99"))
            .unwrap();
        assert!(
            last.rect[1] + last.rect[3] <= SIZE.1,
            "last task is reachable by scrolling"
        );
        let empty = runtime(Vec::new(), false);
        assert!(empty.find("no-todos").is_some());
        assert!(empty.find("peek-plan-today").is_some());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn option_tap_detector_fires_only_on_clean_short_option_tap() {
        use crate::app_settings::{OPTION_KEY_MASK, SHIFT_KEY_MASK};
        use crate::audio::CaptureInstant;
        use crate::suppression::InputEvent;

        let base = CaptureInstant::ZERO;
        let at = |ms: u64| base + Duration::from_millis(ms);
        let mut detector = OptionTapDetector::default();

        // Clean single tap (120 ms) triggers immediately when double-tap lock is off.
        assert!(!detector.observe(InputEvent::Flags(OPTION_KEY_MASK), at(0), false));
        assert!(detector.observe(InputEvent::Flags(0), at(120), false));

        // Holding Option (>= 280 ms, dictation hold) does NOT trigger.
        assert!(!detector.observe(InputEvent::Flags(OPTION_KEY_MASK), at(500), false));
        assert!(!detector.observe(InputEvent::Flags(0), at(900), false));

        // Option + Shift chord (e.g. ⌥⇧V) does NOT trigger.
        assert!(!detector.observe(InputEvent::Flags(OPTION_KEY_MASK), at(1_000), false));
        assert!(!detector.observe(
            InputEvent::Flags(OPTION_KEY_MASK | SHIFT_KEY_MASK),
            at(1_040),
            false
        ));
        assert!(!detector.observe(InputEvent::Flags(OPTION_KEY_MASK), at(1_080), false));
        assert!(!detector.observe(InputEvent::Flags(0), at(1_120), false));

        // Option + key (e.g. ⌥←) does NOT trigger.
        assert!(!detector.observe(InputEvent::Flags(OPTION_KEY_MASK), at(1_500), false));
        assert!(!detector.observe(
            InputEvent::Key {
                code: 123,
                down: true,
                flags: OPTION_KEY_MASK,
            },
            at(1_540),
            false
        ));
        assert!(!detector.observe(InputEvent::Flags(0), at(1_600), false));

        // Option + mouse click does NOT trigger.
        assert!(!detector.observe(InputEvent::Flags(OPTION_KEY_MASK), at(2_000), false));
        assert!(!detector.observe(InputEvent::MouseDown, at(2_050), false));
        assert!(!detector.observe(InputEvent::Flags(0), at(2_100), false));

        // Subsequent clean tap works again after contamination clears.
        assert!(!detector.observe(InputEvent::Flags(OPTION_KEY_MASK), at(2_500), false));
        assert!(detector.observe(InputEvent::Flags(0), at(2_620), false));

        // With double-tap lock on: tap -> second Option press within 300 ms cancels toggle (zero flash!).
        assert!(!detector.observe(InputEvent::Flags(OPTION_KEY_MASK), at(3_000), true));
        assert!(!detector.observe(InputEvent::Flags(0), at(3_100), true));
        assert!(!detector.poll_deferred(at(3_200), false));
        assert!(!detector.observe(InputEvent::Flags(OPTION_KEY_MASK), at(3_250), true));
        assert!(!detector.observe(InputEvent::Flags(0), at(3_330), true));
        assert!(!detector.poll_deferred(at(3_700), false));

        // With double-tap lock on: clean single tap with no second press fires after 300 ms.
        assert!(!detector.observe(InputEvent::Flags(OPTION_KEY_MASK), at(4_000), true));
        assert!(!detector.observe(InputEvent::Flags(0), at(4_100), true));
        assert!(!detector.poll_deferred(at(4_350), false));
        assert!(detector.poll_deferred(at(4_400), false));
        assert!(!detector.poll_deferred(at(4_500), false));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn option_tap_and_hold_behave_consistently_across_100ms_200ms_and_400ms() {
        use crate::app_settings::OPTION_KEY_MASK;
        use crate::audio::CaptureInstant;
        use crate::suppression::InputEvent;

        // Exercises the real `HoldPreviewGate::tick` (used by `Driver::tick`) together
        // with `OptionTapDetector` at 100, 200, 279, 280, and 400 ms from both closed and pinned.
        for initial_pinned in [false, true] {
            for hold_ms in [100u64, 200, 279, 280, 400] {
                let mut gate = HoldPreviewGate::default();
                let mut detector = OptionTapDetector::default();
                let mut pinned = initial_pinned;
                let t0_cap = CaptureInstant::ZERO;
                let t0_wall = Instant::now();
                let delta = Duration::from_millis(hold_ms);

                assert!(!detector.observe(InputEvent::Flags(OPTION_KEY_MASK), t0_cap, false));
                assert!(
                    !gate.tick(true, t0_wall),
                    "preview must never show at press instant"
                );
                let preview_shown_during_hold = gate.tick(true, t0_wall + delta);
                let tapped = detector.observe(InputEvent::Flags(0), t0_cap + delta, false);
                if tapped {
                    pinned = !pinned;
                }
                assert!(
                    !gate.tick(false, t0_wall + delta),
                    "preview hides immediately on Option release"
                );

                if hold_ms < 280 {
                    assert!(
                        !preview_shown_during_hold,
                        "{hold_ms}ms release (initial_pinned={initial_pinned}) must not flash hold preview"
                    );
                    assert!(
                        tapped,
                        "{hold_ms}ms release (initial_pinned={initial_pinned}) must fire tap toggle"
                    );
                    assert_eq!(
                        pinned, !initial_pinned,
                        "{hold_ms}ms tap must flip pinned from {initial_pinned} to {}",
                        !initial_pinned
                    );
                } else {
                    assert!(
                        preview_shown_during_hold,
                        "{hold_ms}ms hold (initial_pinned={initial_pinned}) must activate HoldPreviewGate"
                    );
                    assert!(
                        !tapped,
                        "{hold_ms}ms hold (initial_pinned={initial_pinned}) must not fire tap toggle"
                    );
                    assert_eq!(
                        pinned, initial_pinned,
                        "{hold_ms}ms hold must leave pinned unchanged ({initial_pinned})"
                    );
                }
            }
        }
    }

    #[test]
    fn peek_copy_and_actions_adapt_cleanly_to_option_mode_and_empty_draft_states() {
        let footer = |rt: &Runtime<Peek>| {
            rt.elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some("peek-footer"))
                .and_then(|_| {
                    rt.elements()
                        .into_iter()
                        .rev()
                        .find(|e| e.text.is_some())
                        .and_then(|e| e.text)
                })
                .unwrap_or_default()
        };

        // Pinned empty state in TapOnly hides duplicate header button and shows close hint.
        let rt_empty_tap = runtime_with(
            Peek {
                shown: true,
                pinned: true,
                committed: false,
                option_mode: OptionPeekMode::TapOnly,
                date_label: "Fri, Nov 20".into(),
                ..Default::default()
            },
            true,
        );
        assert!(rt_empty_tap.find("peek-plan-today").is_some());
        assert!(
            rt_empty_tap.find("peek-open").is_none(),
            "empty state hides duplicate header Open planner button"
        );
        assert_eq!(footer(&rt_empty_tap), "⌥ or × closes");

        // Pinned empty state in MenuBarOnly shows honest close hint without claiming Esc works unfocused.
        let rt_empty_menu = runtime_with(
            Peek {
                shown: true,
                pinned: true,
                committed: false,
                option_mode: OptionPeekMode::MenuBarOnly,
                date_label: "Fri, Nov 20".into(),
                ..Default::default()
            },
            false,
        );
        assert_eq!(footer(&rt_empty_menu), "Menu bar or × closes");

        // Pinned committed state in MenuBarOnly shows × close hint until a step takes focus.
        let rt_menu = runtime_with(
            Peek {
                shown: true,
                pinned: true,
                committed: true,
                option_mode: OptionPeekMode::MenuBarOnly,
                date_label: "Fri, Nov 20".into(),
                steps: vec![PeekStep {
                    index: 0,
                    text: "Ship clean build".into(),
                    done: false,
                }],
                ..Default::default()
            },
            true,
        );
        assert!(rt_menu.find("peek-open").is_some());
        assert_eq!(footer(&rt_menu), "Click a step to check off · × closes");

        // Once a step is selected in MenuBarOnly, keyboard focus is active and Esc works.
        let rt_menu_selected = runtime_with(
            Peek {
                shown: true,
                pinned: true,
                committed: true,
                option_mode: OptionPeekMode::MenuBarOnly,
                date_label: "Fri, Nov 20".into(),
                steps: vec![PeekStep {
                    index: 0,
                    text: "Ship clean build".into(),
                    done: false,
                }],
                selected: Some(0),
                ..Default::default()
            },
            true,
        );
        assert_eq!(
            footer(&rt_menu_selected),
            "↑↓ + Space to check off · Esc or × closes"
        );

        // Draft state hides duplicate header Open planner button.
        let rt_draft = runtime_with(
            Peek {
                shown: true,
                pinned: true,
                committed: false,
                option_mode: OptionPeekMode::TapOnly,
                date_label: "Fri, Nov 20".into(),
                steps: vec![PeekStep {
                    index: 0,
                    text: "Draft step".into(),
                    done: false,
                }],
                ..Default::default()
            },
            true,
        );
        assert!(rt_draft.find("peek-finish-draft").is_some());
        assert!(
            rt_draft.find("peek-open").is_none(),
            "draft state hides duplicate header Open planner button"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn option_tap_closes_pinned_overlay_even_when_option_starts_provisional_dictation() {
        use crate::app_settings::{HotkeyBinding, HotkeyModifiers, ModifierSide, OPTION_KEY_MASK};
        use crate::audio::CaptureInstant;
        use crate::suppression::{DictationHotkey, HotkeyAction, InputEvent};

        let binding = HotkeyBinding {
            modifiers: HotkeyModifiers {
                option: Some(ModifierSide::Either),
                ..Default::default()
            },
            key: None,
        }
        .runtime();
        let at = |ms: u64| CaptureInstant::ZERO + Duration::from_millis(ms);

        // 1. Bare-Option Hold mode (both double_tap_lock = false and true):
        //    pinned -> Option down (provisional dictation starts) -> tick while busy
        //    -> Option up at 120 ms -> list cleanly closes (pinned == false, no reopen!).
        for double_tap_lock in [false, true] {
            let mut hotkey = DictationHotkey::with_binding(false, at(0), double_tap_lock, binding);
            let mut tap = OptionTapDetector::default();
            let mut pin = PeekPinState {
                pinned: true,
                intentional_dictation: false,
            };

            let down = InputEvent::Flags(OPTION_KEY_MASK);
            assert!(!tap.observe(down, at(10), double_tap_lock));
            assert_eq!(hotkey.process(down, at(10)), Some(HotkeyAction::Start));
            // Timer tick during the provisional press must NOT unpin Today's Steps.
            assert!(
                !pin.on_tick(true),
                "provisional capture must not count as intentional dictation (double_tap_lock={double_tap_lock})"
            );
            assert!(
                pin.pinned,
                "pinned stays true while provisional tap is down"
            );

            let up = InputEvent::Flags(0);
            let tapped_now = tap.observe(up, at(130), double_tap_lock);
            assert_eq!(hotkey.process(up, at(130)), Some(HotkeyAction::Finish));
            assert!(!hotkey.is_recording());
            assert!(!pin.on_tick(false));
            if tapped_now || tap.poll_deferred(at(130 + 300), hotkey.is_recording()) {
                pin.on_option_tap();
            }
            assert!(
                !pin.pinned,
                "120 ms Option tap while pinned must close the overlay (double_tap_lock={double_tap_lock})"
            );
        }

        // 2. Pinned -> real Option hold (>= 300 ms) becomes intentional -> hides and stays closed on release.
        {
            let mut hotkey = DictationHotkey::with_binding(false, at(0), true, binding);
            let mut tap = OptionTapDetector::default();
            let mut pin = PeekPinState {
                pinned: true,
                intentional_dictation: false,
            };
            assert!(!tap.observe(InputEvent::Flags(OPTION_KEY_MASK), at(10), true));
            assert_eq!(
                hotkey.process(InputEvent::Flags(OPTION_KEY_MASK), at(10)),
                Some(HotkeyAction::Start)
            );
            // At 310 ms, capture becomes intentional -> hides pinned list.
            pin.on_dictation_intentional();
            assert!(pin.on_tick(true));
            assert!(!pin.pinned, "intentional hold unpins Today's Steps");
            // Release at 600 ms is >= 280 ms so tap does not fire and list stays closed.
            assert!(!tap.observe(InputEvent::Flags(0), at(600), true));
            assert_eq!(
                hotkey.process(InputEvent::Flags(0), at(600)),
                Some(HotkeyAction::Finish)
            );
            assert!(!tap.poll_deferred(at(1_000), false));
            assert!(!pin.on_tick(false));
            assert!(
                !pin.pinned,
                "list stays closed after real dictation finishes"
            );
        }

        // 3. Pinned -> double-tap Option to lock dictation -> cancels pending toggle,
        //    and hides once locked capture becomes intentional.
        {
            let mut hotkey = DictationHotkey::with_binding(false, at(0), true, binding);
            let mut tap = OptionTapDetector::default();
            let mut pin = PeekPinState {
                pinned: true,
                intentional_dictation: false,
            };
            assert!(!tap.observe(InputEvent::Flags(OPTION_KEY_MASK), at(10), true));
            assert_eq!(
                hotkey.process(InputEvent::Flags(OPTION_KEY_MASK), at(10)),
                Some(HotkeyAction::Start)
            );
            assert!(!tap.observe(InputEvent::Flags(0), at(90), true));
            assert_eq!(
                hotkey.process(InputEvent::Flags(0), at(90)),
                Some(HotkeyAction::Finish)
            );
            // Second tap within 300 ms cancels pending tap and locks dictation.
            assert!(!tap.observe(InputEvent::Flags(OPTION_KEY_MASK), at(160), true));
            assert_eq!(
                hotkey.process(InputEvent::Flags(OPTION_KEY_MASK), at(160)),
                Some(HotkeyAction::Start)
            );
            assert!(!tap.observe(InputEvent::Flags(0), at(220), true));
            assert_eq!(hotkey.process(InputEvent::Flags(0), at(220)), None);
            assert!(hotkey.is_recording());
            assert!(!tap.poll_deferred(at(600), hotkey.is_recording()));
            pin.on_dictation_intentional();
            assert!(!pin.pinned);
        }
    }

    #[test]
    fn option_peek_renders() {
        #[cfg(target_os = "linux")]
        let _goldens = super::super::super::tests::Goldens::collect();
        let folder = std::env::var_os("HEX_PLANNER_RENDERS").map(std::path::PathBuf::from);
        if !cfg!(target_os = "linux") && folder.is_none() {
            return;
        }
        #[cfg(target_os = "linux")]
        let gpu = super::super::super::tests::gpu().expect("Planner cards need a GPU");
        #[cfg(not(target_os = "linux"))]
        let gpu = rune::render::Gpu::try_new(rune::render::Gpu::instance(), None).unwrap();
        let sample_steps = || {
            vec![
                PeekStep {
                    index: 0,
                    text: "Finish the smallest useful Planner improvement".into(),
                    done: false,
                },
                PeekStep {
                    index: 1,
                    text: "Review the Rune renders in light and dark mode".into(),
                    done: false,
                },
                PeekStep {
                    index: 2,
                    text: "Install the verified build and keep shortcuts working".into(),
                    done: true,
                },
            ]
        };
        for dark in [false, true] {
            let suffix = if dark { "dark" } else { "light" };
            let mut rt = runtime(sample_steps(), dark);
            let pixels =
                super::super::super::offscreen::draw(&gpu, &mut rt, SIZE.0 as u32, SIZE.1 as u32);
            if let Some(folder) = &folder {
                std::fs::create_dir_all(folder).unwrap();
                rune::render::write_png(
                    &folder.join(format!("planner-todos-{suffix}.png")),
                    SIZE.0 as u32,
                    SIZE.1 as u32,
                    &pixels,
                )
                .unwrap();
            }
            #[cfg(target_os = "linux")]
            super::super::super::tests::check_render(
                "planner-todos",
                dark,
                Some((SIZE.0 as u32, SIZE.1 as u32, pixels)),
            );

            // Also render all pinned and edge states when generating review PNGs.
            if let Some(folder) = &folder {
                type PeekVariant = (&'static str, fn(&mut Peek));
                let variants: [PeekVariant; 5] = [
                    ("planner-todos-pinned", |s| {
                        s.pinned = true;
                        s.selected = None;
                    }),
                    ("planner-todos-pinned-selected", |s| {
                        s.pinned = true;
                        s.selected = Some(0);
                    }),
                    ("planner-todos-all-done", |s| {
                        s.pinned = true;
                        for step in &mut s.steps {
                            step.done = true;
                        }
                    }),
                    ("planner-todos-draft", |s| {
                        s.pinned = true;
                        s.committed = false;
                        for step in &mut s.steps {
                            step.done = false;
                        }
                    }),
                    ("planner-todos-empty", |s| {
                        s.pinned = true;
                        s.committed = false;
                        s.focus.clear();
                        s.steps.clear();
                    }),
                ];
                for (name, configure) in variants {
                    let mut state = Peek {
                        shown: true,
                        pinned: false,
                        committed: true,
                        option_mode: OptionPeekMode::TapOnly,
                        date_label: "Fri, Nov 20".into(),
                        focus: "Make the daily planning overlay feel effortless".into(),
                        steps: sample_steps(),
                        selected: None,
                        on_toggle: None,
                        on_open: None,
                        on_pin: None,
                        #[cfg(target_os = "macos")]
                        handle: None,
                    };
                    configure(&mut state);
                    let mut rt = runtime_with(state, dark);
                    let px = super::super::super::offscreen::draw(
                        &gpu,
                        &mut rt,
                        SIZE.0 as u32,
                        SIZE.1 as u32,
                    );
                    rune::render::write_png(
                        &folder.join(format!("{name}-{suffix}.png")),
                        SIZE.0 as u32,
                        SIZE.1 as u32,
                        &px,
                    )
                    .unwrap();
                }
            }
        }
    }
}
