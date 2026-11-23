//! The local planner ledger. Dates are Eastern calendar days with a 6 AM boundary.

use std::io::{self, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};

const MAX_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const MAX_TEXT: usize = 8_192;
pub(super) const MAX_STEPS: usize = 100;
pub(super) const RECOMMENDED_MAX_STEPS: usize = 5;

pub(super) fn answer(value: &str) -> bool {
    value.len() <= MAX_TEXT && value.trim().chars().filter(|c| c.is_alphanumeric()).count() >= 3
}

pub(super) fn task(value: &str) -> bool {
    value.len() <= MAX_TEXT && value.trim().chars().count() >= 3
}

pub(super) fn plural(count: usize, singular: &str, plural_form: &str) -> String {
    format!(
        "{count} {}",
        if count == 1 { singular } else { plural_form }
    )
}

pub(super) fn planning_day(unix_seconds: i64, eastern_offset: i64) -> i64 {
    (unix_seconds + eastern_offset - 6 * 3_600).div_euclid(86_400)
}

fn civil_from_day(day: i64) -> (i64, u32, u32) {
    // Inverse of history::stats::days_from_civil, in the ledger's Eastern calendar.
    let day = day + 719_468;
    let era = day.div_euclid(146_097);
    let doe = day - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let date = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (mp + if mp < 10 { 3 } else { -9 }) as u32;
    let year = year + i64::from(month <= 2);
    (year, month, date)
}

#[cfg(test)]
pub(super) fn date_label(day: i64) -> String {
    let (year, month, date) = civil_from_day(day);
    format!("{year:04}-{month:02}-{date:02}")
}

pub(super) fn human_date_label(day: i64) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, month, date) = civil_from_day(day);
    // 1970-01-01 (day 0) was a Thursday (index 4).
    let weekday = WEEKDAYS[(day + 4).rem_euclid(7) as usize];
    let month_name = MONTHS[(month.saturating_sub(1) as usize).min(11)];
    format!("{weekday}, {month_name} {date}")
}

pub(super) fn short_weekday(day: i64) -> &'static str {
    const LETTERS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];
    LETTERS[(day + 4).rem_euclid(7) as usize]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DayOutcome {
    Delivered,
    NotYet,
    Planned,
    Skipped,
    Today,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(super) struct Step {
    pub text: String,
    pub done: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(super) struct Day {
    pub date: i64,
    pub yesterday: String,
    pub pending_step: String,
    pub pending_accomplishment: String,
    pub problem: String,
    pub steps: Vec<Step>,
    pub breakdown_started: bool,
    pub deliverable: String,
    pub criteria: String,
    pub committed: bool,
    /// Answered at the next reflection, independently of committing a plan.
    pub delivered: Option<bool>,
    /// Optional note on what got in the way when `delivered == Some(false)`.
    pub blocker: String,
    /// Unfinished steps from the previous plan awaiting Keep / Drop triage.
    pub carry_over: Vec<String>,
}

impl Day {
    pub fn valid(&self) -> bool {
        answer(&self.problem)
            && self.pending_step.trim().is_empty()
            && !self.steps.is_empty()
            && self.steps.len() <= MAX_STEPS
            && self.steps.iter().all(|step| task(&step.text))
            && answer(&self.deliverable)
            && answer(&self.criteria)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(super) struct Week {
    pub date: i64,
    pub reflection: String,
    pub goal: String,
    pub remaining: String,
    pub focus: String,
    pub success: String,
}

impl Week {
    pub fn valid(&self, first: bool) -> bool {
        (first || answer(&self.reflection))
            && [&self.goal, &self.remaining, &self.focus, &self.success]
                .into_iter()
                .all(|value| answer(value))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(super) struct Ledger {
    pub version: u32,
    pub days: Vec<Day>,
    pub weeks: Vec<Week>,
    pub week_draft: Option<Week>,
    pub snoozed_until: u64,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            version: 1,
            days: Vec::new(),
            weeks: Vec::new(),
            week_draft: None,
            snoozed_until: 0,
        }
    }
}

impl Ledger {
    pub fn load(path: &Path) -> io::Result<Self> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("Planner file exceeds 8 MB"));
        }
        let ledger: Self = serde_json::from_slice(&bytes)?;
        if ledger.version != 1 {
            return Err(io::Error::other("Planner file has an unsupported version"));
        }
        let valid_date = |date| (-719_162..=2_932_896).contains(&date);
        let mut dates = std::collections::HashSet::new();
        if ledger.days.iter().any(|day| {
            !valid_date(day.date) || !dates.insert(day.date) || day.steps.len() > MAX_STEPS
        }) || ledger.weeks.iter().any(|week| !valid_date(week.date))
            || ledger
                .week_draft
                .as_ref()
                .is_some_and(|week| !valid_date(week.date))
        {
            return Err(io::Error::other(
                "Planner file contains invalid dates or checklists",
            ));
        }
        Ok(ledger)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::other(
                "Planner file exceeds 8 MB; nothing was replaced",
            ));
        }
        if let Some(parent) = path.parent() {
            crate::app_paths::create_private_dir(parent)?;
        }
        crate::durable::write_private(path, &bytes)
    }

    pub fn today(&mut self, date: i64) -> &mut Day {
        if let Some(index) = self.days.iter().position(|day| day.date == date) {
            return &mut self.days[index];
        }
        self.days.push(Day {
            date,
            ..Day::default()
        });
        self.days.last_mut().unwrap()
    }

    pub fn day_ref(&self, date: i64) -> Option<&Day> {
        self.days.iter().find(|day| day.date == date)
    }

    pub fn previous(&self, date: i64) -> Option<&Day> {
        // Opening and bypassing an empty day must not hide the last real plan.
        self.days
            .iter()
            .filter(|day| {
                day.date < date
                    && (day.committed
                        || !day.steps.is_empty()
                        || !day.deliverable.trim().is_empty())
            })
            .max_by_key(|day| day.date)
    }

    /// Populates `carry_over` once from the previous plan's unfinished steps so the user can
    /// explicitly Keep or Drop each item instead of having old tasks pile up silently.
    pub fn seed_carry_over(&mut self, date: i64) {
        if self.today(date).breakdown_started {
            return;
        }
        let unfinished: Vec<String> = self
            .previous(date)
            .into_iter()
            .flat_map(|day| &day.steps)
            .filter(|step| !step.done && !step.text.trim().is_empty())
            .map(|step| step.text.clone())
            .collect();
        let today = self.today(date);
        if today.steps.is_empty() && today.carry_over.is_empty() {
            today.carry_over = unfinished;
        }
        today.breakdown_started = true;
    }

    pub fn keep_carry_over(&mut self, date: i64, index: usize) -> Option<String> {
        let today = self.today(date);
        if index >= today.carry_over.len() || today.steps.len() >= MAX_STEPS {
            return None;
        }
        let text = today.carry_over.remove(index);
        today.steps.push(Step {
            text: text.clone(),
            done: false,
        });
        Some(text)
    }

    pub fn drop_carry_over(&mut self, date: i64, index: usize) -> bool {
        let today = self.today(date);
        if index >= today.carry_over.len() {
            return false;
        }
        today.carry_over.remove(index);
        true
    }

    pub fn keep_all_carry_over(&mut self, date: i64) -> Vec<String> {
        let today = self.today(date);
        let available = MAX_STEPS.saturating_sub(today.steps.len());
        let take = today.carry_over.len().min(available);
        let kept: Vec<String> = today.carry_over.drain(..take).collect();
        for text in &kept {
            today.steps.push(Step {
                text: text.clone(),
                done: false,
            });
        }
        kept
    }

    pub fn drop_all_carry_over(&mut self, date: i64) {
        self.today(date).carry_over.clear();
    }

    pub fn toggle_step(&mut self, date: i64, index: usize) -> bool {
        let Some(day) = self.days.iter_mut().find(|day| day.date == date) else {
            return false;
        };
        let Some(step) = day.steps.get_mut(index) else {
            return false;
        };
        step.done = !step.done;
        true
    }

    /// Returns the last 7 calendar days ending on `date`, for the weekly visual strip.
    pub fn recent_seven_days(&self, date: i64) -> Vec<(i64, DayOutcome)> {
        ((date - 6)..=date)
            .map(|d| {
                let outcome = if d == date {
                    match self.day_ref(d) {
                        Some(day) if day.committed => DayOutcome::Planned,
                        _ => DayOutcome::Today,
                    }
                } else {
                    match self.day_ref(d) {
                        Some(day) if day.delivered == Some(true) => DayOutcome::Delivered,
                        Some(day) if day.delivered == Some(false) => DayOutcome::NotYet,
                        Some(day) if day.committed => DayOutcome::Planned,
                        _ => DayOutcome::Skipped,
                    }
                };
                (d, outcome)
            })
            .collect()
    }

    pub fn week_due(&self, date: i64) -> bool {
        self.weeks.last().is_none_or(|week| date - week.date >= 7)
    }

    pub fn due(&self, date: i64, now: u64) -> bool {
        now >= self.snoozed_until
            && (self.week_due(date)
                || !self
                    .days
                    .iter()
                    .any(|day| day.date == date && day.committed))
    }

    pub fn start_week(&mut self, date: i64) {
        if self.week_draft.is_none() {
            self.week_draft = Some(Week {
                date,
                ..self.weeks.last().cloned().unwrap_or_default()
            });
            if let Some(draft) = &mut self.week_draft {
                draft.reflection.clear();
            }
        }
    }

    pub fn finish_week(&mut self, date: i64) -> Result<(), &'static str> {
        let Some(draft) = &self.week_draft else {
            return Err("Set your weekly focus first.");
        };
        if !draft.valid(self.weeks.is_empty()) {
            return Err("Fill in each field with a short answer before continuing.");
        }
        let mut week = self.week_draft.take().unwrap();
        week.date = date;
        self.weeks.push(week);
        Ok(())
    }

    pub fn commit(&mut self, date: i64) -> Result<(), &'static str> {
        if self.week_due(date) {
            return Err("Finish your weekly review first.");
        }
        let has_previous = self.previous(date).is_some();
        if has_previous && self.previous(date).unwrap().delivered.is_none() {
            return Err("Answer whether you met yesterday's criteria first.");
        }
        let day = self.today(date);
        if !day.valid() {
            return Err("Add a focus, deliverable, done criteria, and at least one step.");
        }
        day.committed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
