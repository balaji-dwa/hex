//! Planner's opt-in daily ritual. The ledger and view are portable; macOS owns presentation.

#![cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rune::{Key, KeyEvent, TextEdit, Tx};

use super::controls::Paint;
use crate::app_settings::{AppSettings, HotkeyBinding};
use model::{Day, Ledger, Step};

#[cfg(target_os = "macos")]
pub(crate) mod macos;
mod model;
pub(crate) mod peek;
#[cfg(test)]
mod tests;
mod view;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slide {
    Progress,
    Goal,
    Focus,
    Picture,
    Yesterday,
    Today,
    Outcome,
    Breakdown,
    DayView,
}

impl Slide {
    fn title(self) -> &'static str {
        match self {
            Self::Progress => "Your week in review",
            Self::Goal => "Long-term direction",
            Self::Focus => "This week’s focus",
            Self::Picture => "Big picture",
            Self::Yesterday => "Yesterday’s check-in",
            Self::Today => "What’s your one focus today?",
            Self::Outcome => "Define done",
            Self::Breakdown => "Break it into small steps",
            Self::DayView => "Your day is set",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Progress => "Look at what shipped, then capture one lesson for next week.",
            Self::Goal => "Keep your overarching goal steady, or refine it if things shifted.",
            Self::Focus => "Pick the main theme for the next seven days before planning today.",
            Self::Picture => {
                "Take ten seconds to reconnect with your goal before choosing today’s work."
            }
            Self::Yesterday => {
                "Close the loop on your last plan so unfinished work doesn’t linger."
            }
            Self::Today => "Pick one problem or piece that would make today feel like a win.",
            Self::Outcome => {
                "Decide what finished looks like before listing tasks so scope stays tight."
            }
            Self::Breakdown => "Keep today to 3–5 concrete steps so you stay calm and on track.",
            Self::DayView => {
                "Check off steps here, tap or hold ⌥ Option, or use Today’s Steps in the menu bar."
            }
        }
    }
}

/// How Option interacts with Today's Steps given the user's dictation shortcut and mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum OptionPeekMode {
    /// Dictation doesn't use bare Option: tap Option to toggle pinned, hold Option to peek, or use menu bar.
    #[default]
    TapOrHold,
    /// Dictation uses bare Option in Hold mode: tap Option to toggle pinned (hold stays for dictation), or use menu bar.
    TapOnly,
    /// Dictation uses bare Option in Toggle mode: Option tap starts/stops dictation, so Today's Steps opens from the menu bar.
    MenuBarOnly,
}

fn binding_is_bare_option(binding: &HotkeyBinding) -> bool {
    let m = binding.modifiers;
    binding.key.is_none()
        && m.option.is_some()
        && m.control.is_none()
        && m.shift.is_none()
        && m.command.is_none()
        && !m.function
}

pub(super) fn dictation_uses_bare_option(settings: &AppSettings) -> bool {
    binding_is_bare_option(&settings.dictation_hotkey)
        || settings
            .second_dictation_hotkey
            .as_ref()
            .is_some_and(binding_is_bare_option)
}

pub(super) fn option_peek_mode(settings: &AppSettings) -> OptionPeekMode {
    if !dictation_uses_bare_option(settings) {
        OptionPeekMode::TapOrHold
    } else if settings.shortcut_mode == crate::dictation::ShortcutMode::Toggle {
        OptionPeekMode::MenuBarOnly
    } else {
        OptionPeekMode::TapOnly
    }
}

pub(crate) struct Planner {
    ledger: Ledger,
    date: i64,
    slide: Slide,
    fields: [TextEdit; 10],
    steps: Vec<TextEdit>,
    visible: bool,
    reopened: bool,
    /// Snapshot of today's committed day while editing an already-committed plan,
    /// so cancelling restores a valid committed plan instead of saving a broken one.
    edit_backup: Option<Day>,
    /// Previous day's checkbox states before clicking "Yes, done", restored if switched to "Not yet".
    yesterday_steps_backup: Option<Vec<bool>>,
    /// How Option interacts with Today's Steps under the current dictation shortcut and mode.
    option_mode: OptionPeekMode,
    path: Option<PathBuf>,
    unreadable: bool,
    error: Option<String>,
    login_note: Option<String>,
    save_at: Option<Instant>,
    #[cfg(target_os = "macos")]
    panel: Option<rune::OverlayHandle<Self>>,
}

impl Planner {
    fn new(mut ledger: Ledger, date: i64, path: Option<PathBuf>) -> Self {
        ledger.today(date);
        let mut planner = Self {
            ledger,
            date,
            slide: Slide::Picture,
            fields: std::array::from_fn(|_| TextEdit::default()),
            steps: Vec::new(),
            visible: false,
            reopened: false,
            edit_backup: None,
            yesterday_steps_backup: None,
            option_mode: OptionPeekMode::TapOrHold,
            path,
            unreadable: false,
            error: None,
            login_note: None,
            save_at: None,
            #[cfg(target_os = "macos")]
            panel: None,
        };
        planner.restore();
        planner
    }

    fn slides(&self) -> Vec<Slide> {
        if self.ledger.week_due(self.date) {
            if self.ledger.weeks.is_empty() {
                vec![Slide::Goal, Slide::Focus]
            } else {
                vec![Slide::Progress, Slide::Goal, Slide::Focus]
            }
        } else if self.edit_backup.is_some() {
            vec![Slide::Today, Slide::Outcome, Slide::Breakdown]
        } else {
            let mut slides = vec![Slide::Picture];
            if self.ledger.previous(self.date).is_some() {
                slides.push(Slide::Yesterday);
            }
            slides.extend([Slide::Today, Slide::Outcome, Slide::Breakdown]);
            slides
        }
    }

    fn today_committed(&self) -> bool {
        self.ledger
            .day_ref(self.date)
            .is_some_and(|day| day.committed)
    }

    fn reload_day_fields(&mut self) {
        let prev_blocker = self
            .ledger
            .previous(self.date)
            .map(|day| day.blocker.clone())
            .unwrap_or_default();
        let day = self.ledger.today(self.date);
        self.fields[0] = TextEdit::new(prev_blocker);
        self.fields[1] = TextEdit::new(day.problem.clone());
        self.fields[2] = TextEdit::new(day.deliverable.clone());
        self.fields[3] = TextEdit::new(day.criteria.clone());
        self.fields[9] = TextEdit::new(day.pending_step.clone());
        self.steps = day
            .steps
            .iter()
            .map(|step| TextEdit::new(step.text.clone()))
            .collect();
    }

    fn restore(&mut self) {
        self.edit_backup = None;
        self.yesterday_steps_backup = None;
        if self.ledger.week_due(self.date) {
            self.ledger.start_week(self.date);
        }
        self.reload_day_fields();
        if let Some(week) = &self.ledger.week_draft {
            for (index, value) in [
                &week.reflection,
                &week.goal,
                &week.remaining,
                &week.focus,
                &week.success,
            ]
            .into_iter()
            .enumerate()
            {
                self.fields[4 + index] = TextEdit::new(value.clone());
            }
        }
        self.slide = if !self.ledger.week_due(self.date) && self.today_committed() {
            Slide::DayView
        } else {
            self.slides()[0]
        };
    }

    fn edited(&mut self, tx: &mut Tx) {
        self.error = None;
        self.sync();
        // While editing an already-committed plan, hold disk writes until the user
        // explicitly saves valid changes so cancelling or crashing never corrupts today.
        if self.edit_backup.is_none() {
            self.save_at = Some(Instant::now() + Duration::from_millis(250));
            tx.wake_after(Duration::from_millis(250));
        }
    }

    fn sync(&mut self) {
        if let Some(prev_date) = self.ledger.previous(self.date).map(|day| day.date) {
            self.ledger.today(prev_date).blocker = self.fields[0].text.clone();
        }
        let day = self.ledger.today(self.date);
        day.problem = self.fields[1].text.clone();
        day.deliverable = self.fields[2].text.clone();
        day.criteria = self.fields[3].text.clone();
        day.pending_step = self.fields[9].text.clone();
        for (step, edit) in day.steps.iter_mut().zip(&self.steps) {
            step.text = edit.text.clone();
        }
        if let Some(week) = &mut self.ledger.week_draft {
            week.reflection = self.fields[4].text.clone();
            week.goal = self.fields[5].text.clone();
            week.remaining = self.fields[6].text.clone();
            week.focus = self.fields[7].text.clone();
            week.success = self.fields[8].text.clone();
        }
    }

    fn save(&mut self) -> bool {
        self.sync();
        if self.unreadable {
            self.error = Some("Planner could not read its saved file. The original is untouched. Fix planner.json and restart Hex; you can close anytime.".into());
            return false;
        }
        self.save_at = None;
        if let Some(path) = &self.path
            && let Err(error) = self.ledger.save(path)
        {
            self.error = Some(format!(
                "Could not save your plan: {error}. Your draft is still here; try again."
            ));
            return false;
        }
        self.error = None;
        true
    }

    pub(super) fn toggle_today_step(&mut self, index: usize) -> bool {
        if self.unreadable || !self.today_committed() {
            return false;
        }
        if !self.ledger.toggle_step(self.date, index) {
            return false;
        }
        self.save();
        true
    }

    fn reveal(&mut self, manual: bool) {
        self.edit_backup = None;
        self.reopened = manual && !self.ledger.week_due(self.date) && self.today_committed();
        if self.reopened {
            self.reload_day_fields();
            self.slide = Slide::DayView;
        } else if self.slide == Slide::DayView && self.ledger.week_due(self.date) {
            self.slide = self.slides()[0];
        }
        self.visible = true;
        #[cfg(target_os = "macos")]
        macos::visible(true);
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            panel.show();
            panel.request_keys();
        }
    }

    fn hide_panel(&mut self) {
        self.visible = false;
        #[cfg(target_os = "macos")]
        macos::visible(false);
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            panel.hide();
        }
    }

    fn cancel_edit(&mut self, tx: &mut Tx) -> bool {
        let Some(backup) = self.edit_backup.take() else {
            return false;
        };
        *self.ledger.today(self.date) = backup;
        self.reload_day_fields();
        self.error = None;
        self.slide = Slide::DayView;
        tx.focus("planner-content", false);
        true
    }

    fn dismiss(&mut self, now: u64) {
        if let Some(backup) = self.edit_backup.take() {
            *self.ledger.today(self.date) = backup;
            self.reload_day_fields();
            self.error = None;
            self.slide = Slide::DayView;
            self.hide_panel();
            return;
        }
        if self.ledger.due(self.date, u64::MAX) {
            self.ledger.snoozed_until = now.saturating_add(300);
        }
        self.save();
        // Emergency escape remains available even when storage has failed.
        self.hide_panel();
    }

    fn close_committed(&mut self) {
        self.edit_backup = None;
        self.save();
        self.hide_panel();
    }

    fn valid_slide(&self) -> bool {
        let valid = |index: usize| model::answer(&self.fields[index].text);
        match self.slide {
            Slide::Progress => valid(4),
            Slide::Goal => valid(5) && valid(6),
            Slide::Focus => valid(7) && valid(8),
            Slide::Picture | Slide::DayView => true,
            Slide::Yesterday => self
                .ledger
                .previous(self.date)
                .is_none_or(|day| day.delivered.is_some()),
            Slide::Today => valid(1),
            Slide::Outcome => valid(2) && valid(3),
            Slide::Breakdown => {
                !self.steps.is_empty()
                    && self.steps.iter().all(|step| model::task(&step.text))
                    && self.fields[9].text.trim().is_empty()
            }
        }
    }

    fn slide_error(&self) -> &'static str {
        let valid = |index: usize| model::answer(&self.fields[index].text);
        match self.slide {
            Slide::Progress => "Add a brief reflection on what worked or needs to change.",
            Slide::Goal if !valid(5) => "Write your overarching goal first.",
            Slide::Goal => "Add what still needs to happen to reach your goal.",
            Slide::Focus if !valid(7) => "Write what you will focus on this week.",
            Slide::Focus => "Describe what would make this a good week.",
            Slide::Yesterday => "Choose Yes, done or Not yet for your last plan.",
            Slide::Today => "Name the one focus you’re tackling today.",
            Slide::Outcome if !valid(2) => "Write what you plan to deliver today.",
            Slide::Outcome => "Write how you’ll know it’s done.",
            Slide::Breakdown if !self.fields[9].text.trim().is_empty() => {
                "Press Enter or Add to keep your typed step, or clear the box first."
            }
            Slide::Breakdown if self.steps.is_empty() => {
                "Add at least one concrete step (or keep one from yesterday)."
            }
            Slide::Breakdown => "Each step needs at least three characters.",
            Slide::Picture | Slide::DayView => "",
        }
    }

    fn focus_first_invalid(&self, tx: &mut Tx) {
        let valid = |index: usize| model::answer(&self.fields[index].text);
        match self.slide {
            Slide::Progress if !valid(4) => tx.focus("answer-4", false),
            Slide::Goal if !valid(5) => tx.focus("answer-5", false),
            Slide::Goal if !valid(6) => tx.focus("answer-6", false),
            Slide::Focus if !valid(7) => tx.focus("answer-7", false),
            Slide::Focus if !valid(8) => tx.focus("answer-8", false),
            Slide::Today if !valid(1) => tx.focus("answer-1", false),
            Slide::Outcome if !valid(2) => tx.focus("answer-2", false),
            Slide::Outcome if !valid(3) => tx.focus("answer-3", false),
            Slide::Breakdown => {
                if !self.fields[9].text.trim().is_empty() || self.steps.is_empty() {
                    tx.focus("new-step", false);
                } else if let Some(idx) = self.steps.iter().position(|s| !model::task(&s.text)) {
                    tx.focus(format!("step-field-{idx}"), false);
                }
            }
            _ => {}
        }
    }

    fn advance(&mut self, tx: &mut Tx) {
        if self.slide == Slide::DayView {
            self.close_committed();
            return;
        }
        self.sync();
        if !self.valid_slide() {
            self.error = Some(self.slide_error().into());
            self.focus_first_invalid(tx);
            return;
        }
        let slides = self.slides();
        let index = slides
            .iter()
            .position(|slide| *slide == self.slide)
            .unwrap_or(0);
        if index + 1 < slides.len() {
            self.slide = slides[index + 1];
            if self.slide == Slide::Breakdown {
                self.ledger.seed_carry_over(self.date);
                self.sync_steps_from_ledger();
            }
            self.error = None;
            if self.edit_backup.is_none() {
                self.save();
            }
        } else {
            let before = self.ledger.clone();
            let result = if self.slide == Slide::Focus {
                self.ledger.finish_week(self.date)
            } else {
                self.ledger.commit(self.date)
            };
            if let Err(error) = result {
                self.error = Some(error.into());
                return;
            }
            if !self.save() {
                self.ledger = before;
                return;
            }
            if self.slide == Slide::Focus {
                self.slide = Slide::Picture;
            } else {
                let was_editing = self.edit_backup.take().is_some();
                self.reopened = was_editing;
                self.slide = Slide::DayView;
            }
        }
        tx.focus("planner-content", false);
    }

    fn back(&mut self, tx: &mut Tx) {
        if self.slide == Slide::DayView {
            self.edit_plan(tx);
            return;
        }
        let slides = self.slides();
        if let Some(index) = slides.iter().position(|slide| *slide == self.slide)
            && index > 0
        {
            self.slide = slides[index - 1];
            self.error = None;
            if self.edit_backup.is_none() {
                self.save();
            }
            tx.focus("planner-content", false);
        }
    }

    fn edit_plan(&mut self, tx: &mut Tx) {
        if self.edit_backup.is_none() && self.today_committed() {
            self.edit_backup = Some(self.ledger.today(self.date).clone());
        }
        self.slide = Slide::Today;
        self.error = None;
        tx.focus("planner-content", false);
    }

    fn mark_yesterday_delivered(&mut self, date: i64, delivered: bool, tx: &mut Tx) {
        if delivered {
            if self.yesterday_steps_backup.is_none() {
                self.yesterday_steps_backup = Some(
                    self.ledger
                        .today(date)
                        .steps
                        .iter()
                        .map(|s| s.done)
                        .collect(),
                );
            }
            let prev = self.ledger.today(date);
            prev.delivered = Some(true);
            for step in &mut prev.steps {
                step.done = true;
            }
        } else {
            let prev = self.ledger.today(date);
            prev.delivered = Some(false);
            if let Some(saved) = self.yesterday_steps_backup.take() {
                for (step, done) in prev.steps.iter_mut().zip(saved) {
                    step.done = done;
                }
            }
        }
        self.edited(tx);
    }

    fn toggle_yesterday_step(&mut self, date: i64, index: usize, tx: &mut Tx) {
        let prev = self.ledger.today(date);
        if let Some(step) = prev.steps.get_mut(index) {
            step.done = !step.done;
            // Keep "Yes, done" and unchecked steps consistent: unchecking any step
            // while "Yes, done" is selected flips the status to "Not yet".
            if !step.done && prev.delivered == Some(true) {
                prev.delivered = Some(false);
                self.yesterday_steps_backup = None;
            }
            self.edited(tx);
        }
    }

    fn sync_steps_from_ledger(&mut self) {
        self.steps = self
            .ledger
            .today(self.date)
            .steps
            .iter()
            .map(|step| TextEdit::new(step.text.clone()))
            .collect();
    }

    fn add_step(&mut self, tx: &mut Tx) {
        let value = self.fields[9].text.trim().to_owned();
        if !model::task(&value) {
            self.error = Some("Each step needs at least three characters.".into());
            tx.focus("new-step", false);
            return;
        }
        let day = self.ledger.today(self.date);
        if day.steps.len() >= model::MAX_STEPS {
            self.error = Some("Keep the list to 100 steps.".into());
            return;
        }
        day.steps.push(Step {
            text: value.clone(),
            done: false,
        });
        self.steps.push(TextEdit::new(value));
        self.fields[9] = TextEdit::default();
        self.edited(tx);
    }

    fn keep_carry(&mut self, index: usize, tx: &mut Tx) {
        if let Some(text) = self.ledger.keep_carry_over(self.date, index) {
            self.steps.push(TextEdit::new(text));
            self.edited(tx);
        }
    }

    fn drop_carry(&mut self, index: usize, tx: &mut Tx) {
        if self.ledger.drop_carry_over(self.date, index) {
            self.edited(tx);
        }
    }

    fn keep_all_carry(&mut self, tx: &mut Tx) {
        let kept = self.ledger.keep_all_carry_over(self.date);
        if !kept.is_empty() {
            for text in kept {
                self.steps.push(TextEdit::new(text));
            }
            self.edited(tx);
        }
    }

    fn drop_all_carry(&mut self, tx: &mut Tx) {
        self.ledger.drop_all_carry_over(self.date);
        self.edited(tx);
    }
}

fn on_key(planner: &mut Planner, event: &KeyEvent, tx: &mut Tx) -> bool {
    if tx.key_repeat() {
        return false;
    }
    let in_editor = tx.focused() == Some("input");
    match event.key {
        Key::Escape => {
            if in_editor {
                tx.focus("planner-content", false);
            } else if planner.cancel_edit(tx) {
                return true;
            } else {
                planner.dismiss(crate::history::now_ms() / 1_000);
            }
            true
        }
        Key::Enter if event.mods.cmd || event.mods.ctrl => {
            planner.advance(tx);
            true
        }
        // The focused editor owns its arrows, selections, and IME composition.
        Key::Left | Key::Right if in_editor => false,
        Key::Left => {
            planner.back(tx);
            true
        }
        Key::Right | Key::Enter => {
            planner.advance(tx);
            true
        }
        _ => false,
    }
}

fn poll(planner: &mut Planner, _: f64, _: &mut Tx) -> bool {
    if planner
        .save_at
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        planner.save();
        return true;
    }
    false
}

pub(super) fn pane(ui: &super::Ui, p: Paint) -> rune::El {
    let body = match option_peek_mode(&ui.settings) {
        OptionPeekMode::TapOrHold => {
            "Take a few minutes each morning at 6 AM Eastern to commit to today’s focus. Tap or hold ⌥ Option—or open Today’s Steps from Hex’s menu bar—to view and check off steps anytime. Your plans stay on this Mac."
        }
        OptionPeekMode::TapOnly => {
            "Take a few minutes each morning at 6 AM Eastern to commit to today’s focus. Tap ⌥ Option to toggle Today’s Steps (hold ⌥ Option stays for dictation), or open Today’s Steps from Hex’s menu bar. Your plans stay on this Mac."
        }
        OptionPeekMode::MenuBarOnly => {
            "Take a few minutes each morning at 6 AM Eastern to commit to today’s focus. Open Today’s Steps from Hex’s menu bar anytime to view and check off steps (⌥ Option stays reserved for Toggle dictation). Your plans stay on this Mac."
        }
    };
    super::controls::bare_page("planner-pane")
        .child(
            rune::text("One focus. Small steps. A clear finish.")
                .font(p.k.ty.body_medium)
                .color(p.text()),
        )
        .child(rune::text(body).font(p.k.ty.ui).color(p.muted()).wrap())
        .child(super::controls::button(
            p,
            "open-planner",
            "Open planner",
            super::controls::ButtonKind::Primary,
            |_: &mut super::Ui, _| {
                #[cfg(target_os = "macos")]
                macos::open();
            },
        ))
}
