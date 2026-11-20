//! The local planner ledger. Dates are Eastern calendar days with a 6 AM boundary.

use std::io::{self, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};

const MAX_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const MAX_TEXT: usize = 8_192;
pub(super) const MAX_STEPS: usize = 100;

pub(super) fn answer(value: &str) -> bool {
    value.len() <= MAX_TEXT
        && value
            .split_whitespace()
            .filter(|word| word.chars().any(char::is_alphanumeric))
            .count()
            >= 5
}

pub(super) fn task(value: &str) -> bool {
    value.len() <= MAX_TEXT && value.trim().chars().count() >= 3
}

pub(super) fn planning_day(unix_seconds: i64, eastern_offset: i64) -> i64 {
    (unix_seconds + eastern_offset - 6 * 3_600).div_euclid(86_400)
}

pub(super) fn date_label(day: i64) -> String {
    // Inverse of history::stats::days_from_civil, in the ledger's Eastern calendar.
    let day = day + 719_468;
    let era = day.div_euclid(146_097);
    let doe = day - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let date = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{date:02}")
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
}

impl Day {
    pub fn valid(&self) -> bool {
        answer(&self.yesterday)
            && answer(&self.problem)
            && self.pending_step.trim().is_empty()
            && self.pending_accomplishment.trim().is_empty()
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

    pub fn previous(&self, date: i64) -> Option<&Day> {
        self.days
            .iter()
            .filter(|day| day.date < date)
            .max_by_key(|day| day.date)
    }

    pub fn carry_forward(&mut self, date: i64) {
        if self.today(date).breakdown_started {
            return;
        }
        let unfinished: Vec<_> = self
            .previous(date)
            .into_iter()
            .flat_map(|day| &day.steps)
            .filter(|step| !step.done)
            .cloned()
            .collect();
        let today = self.today(date);
        if today.steps.is_empty() {
            today.steps = unfinished;
        }
        today.breakdown_started = true;
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
            return Err("Set your weekly focus first");
        };
        if !draft.valid(self.weeks.is_empty()) {
            return Err("Each answer needs at least five words");
        }
        let mut week = self.week_draft.take().unwrap();
        week.date = date;
        self.weeks.push(week);
        Ok(())
    }

    pub fn commit(&mut self, date: i64) -> Result<(), &'static str> {
        if self.week_due(date) {
            return Err("Finish your weekly review first");
        }
        let has_previous = self.previous(date).is_some();
        if has_previous && self.previous(date).unwrap().delivered.is_none() {
            return Err("Say whether the previous deliverable was completed");
        }
        let day = self.today(date);
        if !day.valid() {
            return Err("Answers need five words; add at least one step of three characters");
        }
        day.committed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
