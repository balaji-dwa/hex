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
        criteria: "All daily slides save correctly".into(),
        ..Day::default()
    }
}

#[test]
fn validation_accepts_concise_meaningful_answers_and_rejects_empty_or_too_long() {
    assert!(!answer(""));
    assert!(!answer("   "));
    assert!(!answer("..."));
    assert!(!answer("ab"));
    assert!(answer("Ship it"));
    assert!(answer("Fix auth bug"));
    assert!(!answer(&"long ".repeat(MAX_TEXT)));
    assert!(!task("  ab  "));
    assert!(task("  中文字  "));
    assert!(task("run"));
}

#[test]
fn dates_and_plurals_format_cleanly() {
    assert_eq!(date_label(0), "1970-01-01");
    assert_eq!(date_label(-1), "1969-12-31");
    assert_eq!(date_label(11_017), "2000-03-01");
    let nov_20 = planning_day(1_795_172_400, -5 * 3_600);
    assert_eq!(date_label(nov_20), "2026-11-20");
    assert_eq!(human_date_label(nov_20), "Fri, Nov 20");
    assert_eq!(short_weekday(nov_20), "F");
    assert_eq!(plural(0, "day", "days"), "0 days");
    assert_eq!(plural(1, "day", "days"), "1 day");
    assert_eq!(plural(3, "step", "steps"), "3 steps");
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
fn breakdown_seeds_carry_over_triage_once_instead_of_silently_copying() {
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
        Step {
            text: "Drop this one".into(),
            done: false,
        },
    ];
    ledger.today(101);
    ledger.today(100).steps[1].done = true;
    ledger.seed_carry_over(101);
    assert!(
        ledger.today(101).steps.is_empty(),
        "unfinished items await explicit Keep / Drop triage"
    );
    assert_eq!(
        ledger.today(101).carry_over,
        vec!["Carry this forward", "Drop this one"]
    );
    assert_eq!(
        ledger.keep_carry_over(101, 0),
        Some("Carry this forward".into())
    );
    assert!(ledger.drop_carry_over(101, 0));
    assert!(ledger.today(101).carry_over.is_empty());
    assert_eq!(
        ledger.today(101).steps,
        vec![Step {
            text: "Carry this forward".into(),
            done: false,
        }]
    );
    assert_eq!(
        ledger.today(100).steps.len(),
        4,
        "keep the original history"
    );
    ledger.today(101).steps.clear();
    ledger.seed_carry_over(101);
    assert!(
        ledger.today(101).carry_over.is_empty(),
        "triaged items do not reappear"
    );
    let mut reread: Ledger = serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
    reread.seed_carry_over(101);
    assert!(
        reread.today(101).carry_over.is_empty(),
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
    ledger.seed_carry_over(102);
    assert_eq!(ledger.today(102).carry_over, vec!["Still to do"]);
}

#[test]
fn toggle_step_flips_done_state_safely() {
    let mut ledger = Ledger::default();
    ledger.days.push(day(100));
    assert!(!ledger.day_ref(100).unwrap().steps[0].done);
    assert!(ledger.toggle_step(100, 0));
    assert!(ledger.day_ref(100).unwrap().steps[0].done);
    assert!(!ledger.toggle_step(100, 99));
    assert!(!ledger.toggle_step(999, 0));
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
