use super::*;

fn week(date: i64) -> Week {
    Week {
        date,
        reflection: "Small steps helped me finish work".into(),
        goal: "Ship a useful personal planning tool".into(),
        remaining: "Finish the overlay and test persistence".into(),
        focus: "Make the daily planning flow usable".into(),
        success: "Complete a plan using only keyboard".into(),
    }
}

fn day(date: i64) -> Day {
    Day {
        date,
        yesterday: "I finished the first planner draft".into(),
        problem: "Make the daily planning overlay usable".into(),
        steps: vec![Step {
            text: "Build the overlay".into(),
            done: false,
        }],
        deliverable: "A working keyboard driven planning overlay".into(),
        criteria: "All five daily slides save correctly".into(),
        ..Day::default()
    }
}

#[test]
fn minimums_count_words_and_trimmed_unicode_characters() {
    assert!(!answer("one two three four"));
    assert!(answer("one  two\nthree\tfour five"));
    assert!(!answer("one two three four ..."));
    assert!(!answer(&"long ".repeat(MAX_TEXT)));
    assert!(!task("  ab  "));
    assert!(task("  中文字  "));
    assert!(task("run"));
}

#[test]
fn dates_match_the_ledgers_calendar() {
    assert_eq!(date_label(0), "1970-01-01");
    assert_eq!(date_label(-1), "1969-12-31");
    assert_eq!(date_label(11_017), "2000-03-01");
    assert_eq!(
        date_label(planning_day(1_795_172_400, -5 * 3_600)),
        "2026-11-20"
    );
}

#[test]
fn eastern_six_am_is_the_only_day_boundary() {
    // 2026-11-20 10:59:59 UTC is 05:59:59 Eastern standard time.
    let reset = 1_795_172_400;
    assert_eq!(
        planning_day(reset - 1, -5 * 3_600) + 1,
        planning_day(reset, -5 * 3_600)
    );
    assert_eq!(
        planning_day(reset, -5 * 3_600),
        planning_day(reset + 23 * 3_600, -5 * 3_600)
    );
    // The fall-back clock change itself never starts another planning day.
    let fall_back = 1_793_512_800; // 2026-11-01 06:00 UTC
    assert_eq!(
        planning_day(fall_back - 1, -4 * 3_600),
        planning_day(fall_back, -5 * 3_600)
    );
}

#[test]
fn weekly_review_is_due_after_seven_calendar_days_even_without_daily_plans() {
    let mut ledger = Ledger::default();
    assert!(ledger.week_due(100));
    ledger.weeks.push(week(100));
    assert!(!ledger.week_due(106));
    assert!(ledger.week_due(107));
    ledger.week_draft = Some(week(100));
    ledger.finish_week(110).unwrap();
    assert_eq!(ledger.weeks.last().unwrap().date, 110);
    assert!(!ledger.week_due(116));
    assert!(ledger.week_due(117));
}

#[test]
fn breakdown_carries_only_tasks_still_unfinished_after_yesterdays_review_once() {
    let mut ledger = Ledger::default();
    ledger.today(100).steps = vec![
        Step {
            text: "Already done".into(),
            done: true,
        },
        Step {
            text: "Done during yesterday's review".into(),
            done: false,
        },
        Step {
            text: "Carry this forward".into(),
            done: false,
        },
    ];
    ledger.today(101);
    ledger.today(100).steps[1].done = true;
    ledger.carry_forward(101);
    assert_eq!(
        ledger.today(101).steps,
        vec![Step {
            text: "Carry this forward".into(),
            done: false
        }]
    );
    assert_eq!(
        ledger.today(100).steps.len(),
        3,
        "keep the original history"
    );
    ledger.today(101).steps.clear();
    ledger.carry_forward(101);
    assert!(
        ledger.today(101).steps.is_empty(),
        "a removed task stays removed"
    );
    let mut reread: Ledger = serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
    reread.carry_forward(101);
    assert!(
        reread.today(101).steps.is_empty(),
        "restart does not seed again"
    );
}

#[test]
fn bypassed_empty_days_do_not_lose_unfinished_tasks() {
    let mut ledger = Ledger::default();
    ledger.today(100).steps.push(Step {
        text: "Still to do".into(),
        done: false,
    });
    ledger.today(101);
    assert_eq!(ledger.previous(102).unwrap().date, 100);
    ledger.carry_forward(102);
    assert_eq!(ledger.today(102).steps[0].text, "Still to do");
}

#[test]
fn commit_requires_all_answers_steps_and_previous_deliverable_status() {
    let mut ledger = Ledger::default();
    ledger.weeks.push(week(100));
    ledger.days.push(day(100));
    ledger.days.push(day(101));
    assert!(ledger.commit(101).is_err());
    ledger.today(100).delivered = Some(false);
    ledger.today(101).steps[0].text = "ab".into();
    assert!(ledger.commit(101).is_err());
    ledger.today(101).steps[0].text = "fix".into();
    ledger.commit(101).unwrap();
    assert!(!ledger.due(101, 1_000));
    assert!(ledger.due(102, 1_000));
    assert!(
        !ledger.today(100).committed,
        "planning is different from delivering"
    );
}

#[test]
fn bypass_and_drafts_survive_restart_and_bad_files_are_preserved() {
    let root = std::env::temp_dir().join(format!("hex-planner-model-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("planner.json");
    let mut ledger = Ledger::default();
    ledger.today(100).pending_step = "my unfinished step".into();
    ledger.snoozed_until = 1_300;
    ledger.save(&path).unwrap();
    let reread = Ledger::load(&path).unwrap();
    assert!(!reread.due(100, 1_299));
    assert!(reread.due(100, 1_300));
    assert_eq!(reread.days[0].pending_step, "my unfinished step");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    std::fs::write(&path, b"{broken").unwrap();
    assert!(Ledger::load(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
    std::fs::write(&path, br#"{"version":99}"#).unwrap();
    assert!(Ledger::load(&path).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
