//! Planner's opt-in daily ritual. The ledger and view are portable; macOS owns presentation.

#![cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rune::{Key, KeyEvent, TextEdit, Tx};

use super::controls::Paint;
use model::{Ledger, Step};

#[cfg(target_os = "macos")]
pub(crate) mod macos;
mod model;
mod peek;
#[cfg(test)]
mod tests;
mod view;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Slide {
    Progress,
    Reflection,
    Goal,
    Remaining,
    Focus,
    Success,
    Picture,
    Yesterday,
    YesterdayTasks,
    Today,
    Breakdown,
    Commit,
    Criteria,
}

impl Slide {
    fn title(self) -> &'static str {
        match self {
            Self::Progress => "This week",
            Self::Reflection => "What worked? What needs to change?",
            Self::Goal => "What are you working toward?",
            Self::Remaining => "What still needs to happen?",
            Self::Focus => "What will you focus on this week?",
            Self::Success => "What would make this a good week?",
            Self::Picture => "Big picture",
            Self::Yesterday => "Did you meet your criteria?",
            Self::YesterdayTasks => "Which steps did you finish?",
            Self::Today => "What are you tackling?",
            Self::Breakdown => "Break it down",
            Self::Commit => "What will you deliver?",
            Self::Criteria => "How will you know it’s done?",
        }
    }
}

pub(crate) struct Planner {
    ledger: Ledger,
    date: i64,
    slide: Slide,
    fields: [TextEdit; 10],
    steps: Vec<TextEdit>,
    visible: bool,
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
                vec![Slide::Goal, Slide::Remaining, Slide::Focus, Slide::Success]
            } else {
                vec![
                    Slide::Progress,
                    Slide::Reflection,
                    Slide::Goal,
                    Slide::Remaining,
                    Slide::Focus,
                    Slide::Success,
                ]
            }
        } else {
            let mut slides = vec![Slide::Picture];
            if self.ledger.previous(self.date).is_some() {
                slides.extend([Slide::Yesterday, Slide::YesterdayTasks]);
            }
            slides.extend([
                Slide::Today,
                Slide::Breakdown,
                Slide::Commit,
                Slide::Criteria,
            ]);
            slides
        }
    }

    fn restore(&mut self) {
        if self.ledger.week_due(self.date) {
            self.ledger.start_week(self.date);
        }
        let day = self.ledger.today(self.date);
        self.fields[0] = TextEdit::new(day.yesterday.clone());
        self.fields[1] = TextEdit::new(day.problem.clone());
        self.fields[2] = TextEdit::new(day.deliverable.clone());
        self.fields[3] = TextEdit::new(day.criteria.clone());
        self.fields[9] = TextEdit::new(day.pending_step.clone());
        self.steps = day
            .steps
            .iter()
            .map(|step| TextEdit::new(step.text.clone()))
            .collect();
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
        self.slide = self.slides()[0];
    }

    fn edited(&mut self, tx: &mut Tx) {
        self.sync();
        self.save_at = Some(Instant::now() + Duration::from_millis(250));
        tx.wake_after(Duration::from_millis(250));
    }

    fn sync(&mut self) {
        let day = self.ledger.today(self.date);
        day.yesterday = self.fields[0].text.clone();
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
            self.error = Some("Planner could not read its saved file. The original is untouched. Fix planner.json and restart Hex; emergency bypass remains available.".into());
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

    fn reveal(&mut self, manual: bool) {
        if manual
            && !self.ledger.week_due(self.date)
            && self
                .ledger
                .days
                .iter()
                .any(|day| day.date == self.date && day.committed)
        {
            self.slide = Slide::Breakdown;
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

    fn dismiss(&mut self, now: u64) {
        if self.ledger.due(self.date, u64::MAX) {
            self.ledger.snoozed_until = now.saturating_add(300);
        }
        self.save();
        // Emergency escape remains available even when storage has failed.
        self.visible = false;
        #[cfg(target_os = "macos")]
        macos::visible(false);
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            panel.hide();
        }
    }

    fn valid_slide(&self) -> bool {
        let valid = |index: usize| model::answer(&self.fields[index].text);
        match self.slide {
            Slide::Reflection => valid(4),
            Slide::Goal => valid(5),
            Slide::Remaining => valid(6),
            Slide::Focus => valid(7),
            Slide::Success => valid(8),
            Slide::Yesterday => self
                .ledger
                .previous(self.date)
                .is_none_or(|day| day.delivered.is_some()),
            Slide::Today => valid(1),
            Slide::Breakdown => {
                !self.steps.is_empty()
                    && self.steps.iter().all(|step| model::task(&step.text))
                    && self.fields[9].text.trim().is_empty()
            }
            Slide::Commit => valid(2),
            Slide::Criteria => valid(3),
            _ => true,
        }
    }

    fn advance(&mut self, tx: &mut Tx) {
        self.sync();
        if !self.valid_slide() {
            self.error = Some(
                match self.slide {
                    Slide::Yesterday => "Choose Yes or Not yet.",
                    Slide::Breakdown if !self.fields[9].text.trim().is_empty() => {
                        "Add or clear the new item first."
                    }
                    Slide::Breakdown if self.steps.is_empty() => "Add at least one step.",
                    Slide::Breakdown => "Each step needs at least three characters.",
                    _ => "Use at least five words; shorten very long answers.",
                }
                .into(),
            );
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
                self.ledger.carry_forward(self.date);
                self.steps = self
                    .ledger
                    .today(self.date)
                    .steps
                    .iter()
                    .map(|step| TextEdit::new(step.text.clone()))
                    .collect();
            }
            self.error = None;
            self.save();
        } else {
            let before = self.ledger.clone();
            let result = if self.slide == Slide::Success {
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
            if self.slide == Slide::Success {
                self.slide = Slide::Picture;
            } else {
                self.visible = false;
                #[cfg(target_os = "macos")]
                macos::visible(false);
                #[cfg(target_os = "macos")]
                if let Some(panel) = &self.panel {
                    panel.hide();
                }
            }
        }
        tx.focus("planner-content", false);
    }

    fn back(&mut self, tx: &mut Tx) {
        let slides = self.slides();
        if let Some(index) = slides.iter().position(|slide| *slide == self.slide)
            && index > 0
        {
            self.slide = slides[index - 1];
            self.save();
            tx.focus("planner-content", false);
        }
    }

    fn add_step(&mut self, tx: &mut Tx) {
        let value = self.fields[9].text.trim().to_owned();
        if !model::task(&value) {
            self.error = Some("Use at least three characters.".into());
            return;
        }
        let day = self.ledger.today(self.date);
        if day.steps.len() >= model::MAX_STEPS {
            self.error = Some("Keep the list to 100 steps".into());
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
}

fn on_key(planner: &mut Planner, event: &KeyEvent, tx: &mut Tx) -> bool {
    if tx.key_repeat() {
        return false;
    }
    match event.key {
        Key::Escape => {
            planner.dismiss(crate::history::now_ms() / 1_000);
            true
        }
        // The focused editor owns its arrows, selections, and IME composition.
        Key::Left | Key::Right if tx.focused() == Some("input") => false,
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

pub(super) fn pane(_: &super::Ui, p: Paint) -> rune::El {
    super::controls::bare_page("planner-pane")
        .child(rune::text("One focus. Small steps. A clear finish.").font(p.k.ty.body_medium).color(p.text()))
        .child(rune::text("Plan each day at 6 AM Eastern, and review the bigger picture every seven days. Your plans stay on this Mac.").font(p.k.ty.ui).color(p.muted()))
        .child(super::controls::button(p, "open-planner", "Open planner", super::controls::ButtonKind::Primary, |_: &mut super::Ui, _| {
            #[cfg(target_os = "macos")]
            macos::open();
        }))
}
