use super::*;
use rune::text::TextSystem;
use rune::{Env, Mods, Runtime};

fn runtime() -> Runtime<Planner> {
    let state = Planner::new(Ledger::default(), 100, None);
    let text = TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
    let mut rt = super::super::offscreen::runtime(
        state,
        view::view,
        (1024., 768.),
        Env {
            scale: 1.,
            system_dark: true,
        },
        text,
    );
    rt.on_key = Some(on_key);
    rt.poll = Some(poll);
    rt.frame(0.);
    rt
}

fn enter(rt: &mut Runtime<Planner>) {
    rt.key(KeyEvent {
        key: Key::Enter,
        mods: Mods::default(),
    });
    rt.frame(0.01);
}

fn fill(rt: &mut Runtime<Planner>, index: usize, value: &str) {
    rt.state.fields[index] = TextEdit::new(value);
    rt.invalidate();
    rt.frame(0.01);
}

#[test]
fn planner_first_week_setup_then_daily_commit_enforces_both_minimums() {
    let mut rt = runtime();
    assert_eq!(rt.state.slide, Slide::Goal);
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Goal);
    for index in [5, 6] {
        fill(&mut rt, index, "Build a useful personal planning tool");
    }
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Remaining);
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Focus);
    for index in [7, 8] {
        fill(&mut rt, index, "Finish one usable daily planning flow");
    }
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Success);
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Picture);
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Today);
    fill(&mut rt, 1, "Make the daily planning overlay usable");
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Breakdown);
    fill(&mut rt, 9, "ab");
    // Press the real Add button to exercise its bound validation.
    tap(&mut rt, "add-step");
    assert!(rt.state.steps.is_empty());
    fill(&mut rt, 9, "Implement the daily planning overlay");
    tap(&mut rt, "add-step");
    assert_eq!(rt.state.steps.len(), 1);
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Commit);
    fill(&mut rt, 2, "A working keyboard driven planning overlay");
    fill(&mut rt, 3, "Every planning slide saves correctly");
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Criteria);
    enter(&mut rt);
    assert!(rt.state.ledger.today(100).committed);
    assert!(!rt.state.visible);
}

fn tap(rt: &mut Runtime<Planner>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.01);
}

#[test]
fn planner_bypass_is_pinned_bottom_right_and_saves_a_partial_answer() {
    let mut rt = runtime();
    fill(&mut rt, 5, "unfinished draft");
    rt.state.visible = true;
    let (x, y) = rt.find("planner-bypass").unwrap();
    assert!(
        x > 850. && y > 700.,
        "subtle bypass stays outside the scrolling form: {x}, {y}"
    );
    tap(&mut rt, "planner-bypass");
    assert!(!rt.state.visible);
    assert_eq!(
        rt.state.ledger.week_draft.as_ref().unwrap().goal,
        "unfinished draft"
    );
    assert!(rt.state.ledger.snoozed_until > crate::history::now_ms() / 1_000);
}

#[test]
fn planner_escape_always_dismisses_even_if_the_file_cannot_be_saved() {
    let mut rt = runtime();
    rt.state.unreadable = true;
    rt.state.visible = true;
    rt.key(KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    });
    rt.frame(0.01);
    assert!(!rt.state.visible);
    assert!(rt.state.ledger.weeks.is_empty());
}

#[test]
fn planner_arrows_edit_text_and_enter_adds_a_checklist_item() {
    let mut rt = runtime();
    tap(&mut rt, "answer-5");
    rt.key(KeyEvent {
        key: Key::Text("Build a useful personal planning tool".into()),
        mods: Mods::default(),
    });
    rt.key(KeyEvent {
        key: Key::Left,
        mods: Mods::default(),
    });
    rt.frame(0.01);
    assert_eq!(rt.state.slide, Slide::Goal);
    assert_eq!(
        rt.state.fields[5].text,
        "Build a useful personal planning tool"
    );
    rt.state.slide = Slide::Breakdown;
    rt.invalidate();
    rt.frame(0.01);
    tap(&mut rt, "new-step");
    rt.key(KeyEvent {
        key: Key::Text("Write the smallest complete change".into()),
        mods: Mods::default(),
    });
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Breakdown);
    assert_eq!(rt.state.steps.len(), 1);
}

#[test]
fn planner_open_questions_are_single_large_editors_without_word_counts() {
    let mut rt = runtime();
    for slide in [
        Slide::Goal,
        Slide::Remaining,
        Slide::Focus,
        Slide::Success,
        Slide::Reflection,
        Slide::Today,
        Slide::Commit,
        Slide::Criteria,
    ] {
        rt.state.slide = slide;
        rt.invalidate();
        rt.settle_all();
        rt.frame(0.01);
        let fields: Vec<_> = rt
            .elements()
            .into_iter()
            .filter(|element| {
                element.path.contains("/question-") && element.path.ends_with("/input")
            })
            .collect();
        assert_eq!(fields.len(), 1, "one editor on {slide:?}");
        let area = rt
            .elements()
            .into_iter()
            .find(|element| {
                element.path.contains("/question-")
                    && element
                        .path
                        .rsplit('/')
                        .next()
                        .is_some_and(|key| key.starts_with("answer-"))
            })
            .unwrap();
        assert!(area.rect[3] > 150., "room to think on {slide:?}");
        let copy: Vec<_> = rt
            .elements()
            .into_iter()
            .filter_map(|element| element.text)
            .collect();
        assert!(!copy.iter().any(|copy| copy.contains("words ·")
            || copy.contains("At least 5")
            || copy.contains("Dictate into")));
    }
    rt.state.slide = Slide::Goal;
    rt.invalidate();
    rt.frame(0.01);
    tap(&mut rt, "answer-5");
    rt.paste("Build a calm and dependable workspace");
    rt.key(KeyEvent {
        key: Key::Enter,
        mods: Mods {
            shift: true,
            ..Default::default()
        },
    });
    rt.frame(0.01);
    assert!(rt.state.fields[5].text.contains('\n'));
    assert_eq!(rt.state.slide, Slide::Goal);
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Remaining);
}

#[test]
fn planner_breakdown_has_no_checkboxes_and_uses_yesterdays_unfinished_steps() {
    let mut rt = runtime();
    rt.state = fixture(Slide::Today);
    // Yesterday's first task is checked, its second task remains unchecked.
    rt.state.ledger.today(rt.state.date).steps.clear();
    rt.state.steps.clear();
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.01);
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Breakdown);
    assert_eq!(rt.state.steps.len(), 1);
    assert_eq!(rt.state.steps[0].text, "Test an interrupted write");
    assert!(rt.find("check-0").is_none());
    let tree = rt.accessibility_json();
    assert!(!tree.to_string().contains("\"check-box\""));
    let elements = rt.elements();
    let saved = elements
        .iter()
        .find(|element| element.path.ends_with("/step-field-0"))
        .unwrap();
    let add = elements
        .iter()
        .find(|element| element.path.ends_with("/new-step"))
        .unwrap();
    assert_eq!(saved.rect[0], add.rect[0]);
    assert_eq!(saved.rect[2], add.rect[2]);
    rt.state.slide = Slide::YesterdayTasks;
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.01);
    assert!(rt.find("previous-step-0").is_some());
    assert!(
        rt.accessibility_json()
            .to_string()
            .contains("\"check-box\"")
    );
}

#[test]
fn planner_yesterdays_deliverable_and_tasks_are_separate_choice_only_pages() {
    let mut rt = runtime();
    rt.state = fixture(Slide::Picture);
    rt.state.ledger.today(20_776).delivered = None;
    rt.state.ledger.today(rt.state.date).steps.clear();
    rt.state.steps.clear();
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.01);
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Yesterday);
    assert!(rt.find("delivered-yes").is_some());
    assert!(rt.find("previous-step-0").is_none());
    assert!(rt.find("new-step").is_none());
    assert!(
        !rt.elements()
            .iter()
            .any(|element| element.path.contains("/question-"))
    );
    tap(&mut rt, "delivered-no");
    assert_eq!(rt.state.slide, Slide::YesterdayTasks);
    assert_eq!(
        rt.state.ledger.previous(rt.state.date).unwrap().delivered,
        Some(false)
    );
    assert!(rt.find("new-step").is_none());
    assert!(
        !rt.elements()
            .iter()
            .any(|element| element.path.contains("/question-"))
    );
    tap(&mut rt, "previous-step-1");
    tap(&mut rt, "planner-next");
    assert_eq!(rt.state.slide, Slide::Today);
    tap(&mut rt, "planner-next");
    assert_eq!(rt.state.slide, Slide::Breakdown);
    assert!(
        rt.state.steps.is_empty(),
        "both of yesterday's tasks are now checked"
    );
}

#[test]
fn planner_failed_save_never_marks_a_week_complete() {
    let mut rt = runtime();
    rt.state.slide = Slide::Success;
    for index in 5..=8 {
        fill(&mut rt, index, "Build a useful personal planning tool");
    }
    rt.state.unreadable = true;
    enter(&mut rt);
    assert!(rt.state.ledger.weeks.is_empty());
    assert!(rt.state.ledger.week_draft.is_some());
    assert_eq!(rt.state.slide, Slide::Success);
}

#[test]
fn planner_weekly_review_requires_reflection_and_then_returns_to_daily_planning() {
    let mut state = fixture(Slide::Progress);
    state.restore();
    let mut tx = Tx::default();
    state.advance(&mut tx);
    assert_eq!(state.slide, Slide::Reflection);
    state.fields[4] = TextEdit::new("one two three four");
    state.advance(&mut tx);
    assert_eq!(state.slide, Slide::Reflection);
    state.fields[4] = TextEdit::new("Smaller steps helped me finish work");
    state.advance(&mut tx);
    assert_eq!(state.slide, Slide::Goal);
    state.advance(&mut tx);
    assert_eq!(state.slide, Slide::Remaining);
    state.advance(&mut tx);
    assert_eq!(state.slide, Slide::Focus);
    state.advance(&mut tx);
    assert_eq!(state.slide, Slide::Success);
    state.advance(&mut tx);
    assert_eq!(state.slide, Slide::Picture);
    assert_eq!(state.ledger.weeks.len(), 2);
    assert_eq!(state.ledger.weeks.last().unwrap().date, state.date);
    assert!(!state.ledger.week_due(state.date + 6));
    assert!(state.ledger.week_due(state.date + 7));
}

#[test]
fn planner_is_an_opt_in_macos_lab_and_a_failed_daily_save_stays_incomplete() {
    let feature = crate::labs::Feature::Planner;
    let settings = crate::app_settings::AppSettings::default();
    assert!(!feature.on(&settings));
    assert_eq!(feature.oses(), &[crate::labs::Os::Macos]);
    let mut state = fixture(Slide::Criteria);
    state.unreadable = true;
    state.advance(&mut Tx::default());
    assert!(!state.ledger.today(state.date).committed);
    state.unreadable = false;
    state.advance(&mut Tx::default());
    assert!(state.ledger.today(state.date).committed);
    state.reveal(true);
    assert_eq!(state.slide, Slide::Breakdown);
    state.dismiss(1_000);
    assert!(!state.ledger.due(state.date, 1_000));
}

#[cfg(target_os = "macos")]
#[test]
fn planner_uses_eastern_six_am_in_winter_summer_and_daylight_saving_transitions() {
    for reset in [1_795_172_400, 1_782_986_400, 1_793_530_800, 1_772_964_000] {
        assert_eq!(
            macos::day_at(reset - 1) + 1,
            macos::day_at(reset),
            "reset at {reset}"
        );
    }
}

fn fixture(slide: Slide) -> Planner {
    let mut ledger = Ledger::default();
    let week = model::Week {
        date: if matches!(
            slide,
            Slide::Progress
                | Slide::Reflection
                | Slide::Goal
                | Slide::Remaining
                | Slide::Focus
                | Slide::Success
        ) {
            20_770
        } else {
            20_777
        },
        reflection: "Small steps helped me finish important work".into(),
        goal: "Build a calm and dependable voice workspace".into(),
        remaining: "Finish the planner and verify the experience".into(),
        focus: "Ship the smallest useful daily planning flow".into(),
        success: "Complete morning planning entirely with the keyboard".into(),
    };
    ledger.weeks.push(week.clone());
    ledger.week_draft = Some(week);
    ledger.days.push(model::Day {
        date: 20_776,
        problem: "Make saved plans survive an app restart".into(),
        deliverable: "A working local ledger with durable saves".into(),
        criteria: "Drafts and bypass timers survive a restart".into(),
        committed: true,
        delivered: Some(true),
        steps: vec![
            model::Step {
                text: "Save the ledger atomically".into(),
                done: true,
            },
            model::Step {
                text: "Test an interrupted write".into(),
                done: false,
            },
        ],
        ..Default::default()
    });
    ledger.days.push(model::Day {
        date: 20_777,
        yesterday: "I finished the planner ledger and persistence checks".into(),
        problem: "Make the daily planning overlay feel effortless".into(),
        deliverable: "A usable keyboard driven morning planning flow".into(),
        criteria: "Every slide saves correctly and accepts dictation".into(),
        steps: vec![
            model::Step {
                text: "Build the daily slides".into(),
                done: true,
            },
            model::Step {
                text: "Verify keyboard navigation and input minimums".into(),
                done: false,
            },
            model::Step {
                text: "Review light and dark screenshots".into(),
                done: false,
            },
        ],
        ..Default::default()
    });
    let mut state = Planner::new(ledger, 20_777, None);
    state.slide = slide;
    state
}

#[test]
fn planner_renders() {
    #[cfg(target_os = "linux")]
    let _goldens = super::super::tests::Goldens::collect();
    let folder = std::env::var_os("HEX_PLANNER_RENDERS").map(std::path::PathBuf::from);
    if !cfg!(target_os = "linux") && folder.is_none() {
        return;
    }
    if let Some(folder) = &folder {
        std::fs::create_dir_all(folder).unwrap();
    }
    #[cfg(target_os = "linux")]
    let Some(gpu) = super::super::tests::gpu() else {
        panic!("Planner goldens require a GPU adapter")
    };
    #[cfg(not(target_os = "linux"))]
    let gpu = rune::render::Gpu::try_new(rune::render::Gpu::instance(), None).unwrap();
    for dark in [false, true] {
        for (name, slide) in [
            ("progress", Slide::Progress),
            ("reflection", Slide::Reflection),
            ("goal", Slide::Goal),
            ("remaining", Slide::Remaining),
            ("focus", Slide::Focus),
            ("success", Slide::Success),
            ("picture", Slide::Picture),
            ("yesterday", Slide::Yesterday),
            ("yesterday-tasks", Slide::YesterdayTasks),
            ("today", Slide::Today),
            ("breakdown", Slide::Breakdown),
            ("commit", Slide::Commit),
            ("criteria", Slide::Criteria),
        ] {
            let text = TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
            let mut rt = super::super::offscreen::runtime(
                fixture(slide),
                view::view,
                (1024., 768.),
                Env {
                    scale: 1.,
                    system_dark: dark,
                },
                text,
            );
            let pixels = super::super::offscreen::draw(&gpu, &mut rt, 1024, 768);
            if let Some(folder) = &folder {
                rune::render::write_png(
                    &folder.join(format!(
                        "planner-{name}-{}.png",
                        if dark { "dark" } else { "light" }
                    )),
                    1024,
                    768,
                    &pixels,
                )
                .unwrap();
            }
            #[cfg(target_os = "linux")]
            super::super::tests::check_render(
                &format!("planner-{name}"),
                dark,
                Some((1024, 768, pixels)),
            );
        }
    }
}
