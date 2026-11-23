use super::*;
use rune::text::TextSystem;
use rune::{Env, Mods, Runtime};

fn runtime() -> Runtime<Planner> {
    let state = Planner::new(Ledger::default(), 20_777, None);
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

fn cmd_enter(rt: &mut Runtime<Planner>) {
    rt.key(KeyEvent {
        key: Key::Enter,
        mods: Mods {
            cmd: true,
            ..Default::default()
        },
    });
    rt.frame(0.01);
}

fn fill(rt: &mut Runtime<Planner>, index: usize, value: &str) {
    rt.state.fields[index] = TextEdit::new(value);
    rt.invalidate();
    rt.frame(0.01);
}

fn tap(rt: &mut Runtime<Planner>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.01);
}

#[test]
fn planner_first_week_setup_then_daily_commit_shows_day_view_recap() {
    let mut rt = runtime();
    rt.state.visible = true;
    assert_eq!(rt.state.slide, Slide::Goal);
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Goal);
    assert!(rt.state.error.is_some());
    fill(&mut rt, 5, "Build a calm personal workspace");
    fill(&mut rt, 6, "Polish the morning planning flow");
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Focus);
    fill(&mut rt, 7, "Ship daily planning");
    fill(&mut rt, 8, "Use it every morning");
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Picture);
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Today);
    fill(&mut rt, 1, "Ship checkable Option todos");
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Outcome);
    fill(&mut rt, 2, "Checkable Option task cards");
    fill(&mut rt, 3, "Clicking a task checks it off and saves");
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Breakdown);
    fill(&mut rt, 9, "ab");
    tap(&mut rt, "add-step");
    assert!(rt.state.steps.is_empty());
    fill(&mut rt, 9, "Make Option cards interactive");
    tap(&mut rt, "add-step");
    assert_eq!(rt.state.steps.len(), 1);
    tap(&mut rt, "planner-next");
    assert!(rt.state.ledger.today(20_777).committed);
    assert_eq!(
        rt.state.slide,
        Slide::DayView,
        "committing lands on the Your Day recap"
    );
    assert!(rt.state.visible, "stays visible for closing confirmation");
    tap(&mut rt, "today-step-0");
    assert!(
        rt.state.ledger.today(20_777).steps[0].done,
        "steps are checkable inside DayView"
    );
    tap(&mut rt, "planner-next");
    assert!(!rt.state.visible, "Start the day closes the planner");
}

#[test]
fn planner_bypass_is_pinned_bottom_right_and_saves_a_partial_answer() {
    let mut rt = runtime();
    fill(&mut rt, 5, "unfinished draft");
    rt.state.visible = true;
    let (x, y) = rt.find("planner-bypass").unwrap();
    assert!(
        x > 800. && y > 700.,
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
fn planner_escape_exits_editor_first_and_always_dismisses_on_second_press() {
    let mut rt = runtime();
    rt.state.unreadable = true;
    rt.state.visible = true;
    tap(&mut rt, "answer-5");
    rt.key(KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    });
    rt.frame(0.01);
    assert!(
        rt.state.visible,
        "first Escape exits the focused editor without aborting the flow"
    );
    rt.key(KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    });
    rt.frame(0.01);
    assert!(
        !rt.state.visible,
        "second Escape dismisses even if unreadable"
    );
    assert!(rt.state.ledger.weeks.is_empty());
}

#[test]
fn planner_arrows_edit_text_and_enter_in_step_does_not_skip_slide() {
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
    rt.state = fixture(Slide::Breakdown);
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.01);
    tap(&mut rt, "step-field-0");
    enter(&mut rt);
    assert_eq!(
        rt.state.slide,
        Slide::Breakdown,
        "Enter inside an existing step never accidentally commits"
    );
    tap(&mut rt, "new-step");
    rt.key(KeyEvent {
        key: Key::Text("Write the smallest complete change".into()),
        mods: Mods::default(),
    });
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Breakdown);
    assert_eq!(rt.state.steps.len(), 4);
    cmd_enter(&mut rt);
    assert_eq!(
        rt.state.slide,
        Slide::DayView,
        "Cmd+Enter commits from Breakdown"
    );
}

#[test]
fn planner_yesterday_merges_criteria_blocker_and_steps_then_triages_carry_over() {
    let mut rt = runtime();
    rt.state = fixture(Slide::Yesterday);
    rt.state.ledger.today(20_776).delivered = None;
    rt.state.ledger.today(rt.state.date).steps.clear();
    rt.state.ledger.today(rt.state.date).breakdown_started = false;
    rt.state.steps.clear();
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.01);
    assert!(rt.find("delivered-yes").is_some());
    assert!(rt.find("delivered-no").is_some());
    assert!(rt.find("previous-step-0").is_some());
    assert!(rt.find("blocker-input").is_none());

    tap(&mut rt, "delivered-no");
    assert_eq!(
        rt.state.slide,
        Slide::Yesterday,
        "selecting Not yet stays on slide so user can note blocker and check steps"
    );
    assert!(
        rt.find("blocker-input").is_some(),
        "Not yet reveals optional blocker prompt"
    );
    tap(&mut rt, "planner-next");
    assert_eq!(rt.state.slide, Slide::Today);
    tap(&mut rt, "planner-next");
    assert_eq!(rt.state.slide, Slide::Outcome);
    tap(&mut rt, "planner-next");
    assert_eq!(rt.state.slide, Slide::Breakdown);
    assert!(
        rt.find("carry-over-triage").is_some(),
        "unfinished yesterday steps appear in triage instead of silently filling steps"
    );
    assert!(rt.state.steps.is_empty());
    tap(&mut rt, "keep-carry-0");
    assert_eq!(rt.state.steps.len(), 1);
    assert_eq!(rt.state.steps[0].text, "Test an interrupted write");
    assert!(rt.find("carry-over-triage").is_none());
}

#[test]
fn planner_yes_done_checks_all_yesterday_steps_and_edit_plan_rolls_back_on_cancel() {
    let mut rt = runtime();
    rt.state = fixture(Slide::Yesterday);
    rt.state.ledger.today(20_776).delivered = None;
    rt.state.ledger.today(20_776).steps[1].done = false;
    rt.invalidate();
    rt.frame(0.01);
    tap(&mut rt, "delivered-yes");
    assert!(
        rt.state.ledger.today(20_776).steps.iter().all(|s| s.done),
        "choosing Yes, done marks yesterday's steps complete"
    );
    tap(&mut rt, "delivered-no");
    assert!(
        !rt.state.ledger.today(20_776).steps[1].done,
        "switching to Not yet restores yesterday's unchecked steps"
    );
    tap(&mut rt, "delivered-yes");
    tap(&mut rt, "previous-step-1");
    assert_eq!(
        rt.state.ledger.today(20_776).delivered,
        Some(false),
        "unchecking a step while Yes, done is selected flips status to Not yet"
    );

    // Pressing Enter in an empty new-step field commits when valid steps exist.
    rt.state = fixture(Slide::Breakdown);
    rt.state.visible = true;
    rt.invalidate();
    rt.frame(0.01);
    tap(&mut rt, "new-step");
    enter(&mut rt);
    assert_eq!(
        rt.state.slide,
        Slide::DayView,
        "Enter in empty new-step commits when at least one step exists"
    );

    // Editing a committed plan uses a backup copy and rolls back safely on Escape.
    rt.state = fixture(Slide::DayView);
    rt.state.visible = true;
    rt.invalidate();
    rt.frame(0.01);
    let original_steps = rt.state.ledger.today(20_777).steps.clone();
    tap(&mut rt, "planner-edit");
    assert_eq!(rt.state.slide, Slide::Today);
    assert!(rt.state.edit_backup.is_some());
    rt.state.slide = Slide::Breakdown;
    rt.state.steps.clear();
    rt.state.ledger.today(20_777).steps.clear();
    rt.invalidate();
    rt.frame(0.01);
    rt.key(KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    });
    rt.frame(0.01);
    assert_eq!(rt.state.slide, Slide::DayView);
    assert!(rt.state.edit_backup.is_none());
    assert_eq!(
        rt.state.ledger.today(20_777).steps,
        original_steps,
        "cancelling edit restores the committed plan intact"
    );
}

#[test]
fn planner_detects_bare_option_dictation_hotkey_and_moves_focus_on_enter() {
    let mut settings = crate::app_settings::AppSettings {
        shortcut_mode: crate::dictation::ShortcutMode::Hold,
        dictation_hotkey: crate::app_settings::HotkeyBinding {
            modifiers: crate::app_settings::HotkeyModifiers {
                option: Some(crate::app_settings::ModifierSide::Either),
                ..Default::default()
            },
            key: None,
        },
        ..Default::default()
    };
    assert!(dictation_uses_bare_option(&settings));
    assert_eq!(option_peek_mode(&settings), OptionPeekMode::TapOnly);

    // Bare Option in Toggle mode reserves Option taps for Toggle dictation.
    settings.shortcut_mode = crate::dictation::ShortcutMode::Toggle;
    assert_eq!(option_peek_mode(&settings), OptionPeekMode::MenuBarOnly);

    // Non-bare-Option shortcut (e.g. Option+Shift) supports both tap and hold peek.
    settings.shortcut_mode = crate::dictation::ShortcutMode::Hold;
    settings.dictation_hotkey.modifiers.shift = Some(crate::app_settings::ModifierSide::Either);
    assert!(!dictation_uses_bare_option(&settings));
    assert_eq!(option_peek_mode(&settings), OptionPeekMode::TapOrHold);

    // Adaptive Yesterday footer hint reflects selection and blocker-note focus.
    let mut rt = runtime();
    rt.state = fixture(Slide::Yesterday);
    rt.state.ledger.today(20_776).delivered = None;
    rt.invalidate();
    rt.frame(0.01);
    let hint_text = |rt: &Runtime<Planner>| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("slide-shortcut-hint"))
            .and_then(|e| e.text)
            .unwrap_or_default()
    };
    assert_eq!(hint_text(&rt), "Choose Yes or Not yet to continue");
    tap(&mut rt, "delivered-yes");
    assert_eq!(hint_text(&rt), "Enter to continue");
    tap(&mut rt, "delivered-no");
    assert_eq!(hint_text(&rt), "Enter to continue");
    rt.settle_all();
    rt.frame(1.0);
    tap(&mut rt, "blocker-input");
    rt.frame(0.01);
    assert!(
        rt.state.fields[0].focused,
        "clicking blocker-input should focus fields[0]"
    );
    assert_eq!(hint_text(&rt), "Enter to continue");

    // Enter on first field of a two-field slide moves to the second field without inserting a newline.
    rt.state = fixture(Slide::Outcome);
    rt.invalidate();
    rt.frame(0.01);
    let before = rt.state.fields[2].text.clone();
    tap(&mut rt, "answer-2");
    enter(&mut rt);
    assert_eq!(rt.state.slide, Slide::Outcome);
    assert_eq!(rt.state.fields[2].text, before, "no stray newline inserted");

    // Weekly review (Progress) and Goal keep the exact same header Y with zero layout jump.
    rt.state = fixture(Slide::Progress);
    rt.invalidate();
    rt.frame(0.01);
    let progress_y = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("planner-card"))
        .unwrap()
        .rect[1];
    let progress_bottom = {
        let r = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("planner-card"))
            .unwrap()
            .rect;
        r[1] + r[3]
    };
    rt.state = fixture(Slide::Goal);
    rt.invalidate();
    rt.frame(0.01);
    let goal_y = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("planner-card"))
        .unwrap()
        .rect[1];
    assert_eq!(progress_y, goal_y, "card header never jumps between slides");
    assert!(
        progress_bottom < 720.,
        "weekly review card fits cleanly inside 1024x768 without clipping: {progress_bottom}"
    );

    // Yesterday slide with Not yet + blocker input + 3 steps fits cleanly inside 1024x768.
    rt.state = fixture(Slide::Yesterday);
    rt.state.ledger.today(20_776).delivered = Some(false);
    rt.state.ledger.today(20_776).steps.push(model::Step {
        text: "Verify dark and light screenshots".into(),
        done: false,
    });
    rt.invalidate();
    rt.frame(0.01);
    let yesterday_rect = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("planner-card"))
        .unwrap()
        .rect;
    assert_eq!(
        yesterday_rect[1], goal_y,
        "Yesterday card keeps same header Y"
    );
    assert!(
        yesterday_rect[1] + yesterday_rect[3] < 720.,
        "Yesterday card with blocker and 3 steps fits cleanly inside 1024x768: {}",
        yesterday_rect[1] + yesterday_rect[3]
    );
}

#[test]
fn planner_failed_save_never_marks_a_week_complete() {
    let mut rt = runtime();
    rt.state.slide = Slide::Focus;
    for index in 5..=8 {
        fill(&mut rt, index, "Build a useful personal planning tool");
    }
    rt.state.unreadable = true;
    enter(&mut rt);
    assert!(rt.state.ledger.weeks.is_empty());
    assert!(rt.state.ledger.week_draft.is_some());
    assert_eq!(rt.state.slide, Slide::Focus);
}

#[test]
fn planner_weekly_review_requires_reflection_and_then_returns_to_daily_planning() {
    let mut state = fixture(Slide::Progress);
    state.restore();
    assert_eq!(state.slide, Slide::Progress);
    let mut tx = Tx::default();
    state.fields[4] = TextEdit::new("");
    state.advance(&mut tx);
    assert_eq!(state.slide, Slide::Progress);
    state.fields[4] = TextEdit::new("Smaller steps helped me ship");
    state.advance(&mut tx);
    assert_eq!(state.slide, Slide::Goal);
    state.advance(&mut tx);
    assert_eq!(state.slide, Slide::Focus);
    state.advance(&mut tx);
    assert_eq!(state.slide, Slide::Picture);
    assert_eq!(state.ledger.weeks.len(), 2);
    assert_eq!(state.ledger.weeks.last().unwrap().date, state.date);
    assert!(!state.ledger.week_due(state.date + 6));
    assert!(state.ledger.week_due(state.date + 7));
}

#[test]
fn planner_is_an_opt_in_macos_lab_and_reopen_opens_day_view() {
    let feature = crate::labs::Feature::Planner;
    let settings = crate::app_settings::AppSettings::default();
    assert!(!feature.on(&settings));
    assert_eq!(feature.oses(), &[crate::labs::Os::Macos]);
    let mut state = fixture(Slide::Breakdown);
    state.unreadable = true;
    state.advance(&mut Tx::default());
    assert!(!state.ledger.today(state.date).committed);
    state.unreadable = false;
    state.advance(&mut Tx::default());
    assert!(state.ledger.today(state.date).committed);
    assert_eq!(state.slide, Slide::DayView);
    state.close_committed();
    assert!(!state.visible);
    state.reveal(true);
    assert_eq!(state.slide, Slide::DayView);
    assert!(state.reopened);
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

#[test]
fn planner_saved_answers_wrap_within_the_content_column() {
    let answer = format!(
        "{} https://example.com/{}",
        "Make steady progress on the most important work this week. ".repeat(6),
        "a".repeat(160)
    );
    for width in [640., 1024.] {
        for slide in [
            Slide::Picture,
            Slide::Progress,
            Slide::Yesterday,
            Slide::Outcome,
            Slide::Breakdown,
            Slide::DayView,
        ] {
            let mut state = fixture(slide);
            let week = state.ledger.weeks.last_mut().unwrap();
            week.goal = answer.clone();
            week.focus = answer.clone();
            week.remaining = answer.clone();
            week.success = answer.clone();
            state.ledger.today(20_776).deliverable = answer.clone();
            state.ledger.today(20_776).criteria = answer.clone();
            state.fields[1] = TextEdit::new(&answer);
            state.fields[2] = TextEdit::new(&answer);
            state.fields[3] = TextEdit::new(&answer);
            let text = TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
            let mut rt = super::super::offscreen::runtime(
                state,
                view::view,
                (width, 768.),
                Env {
                    scale: 1.,
                    system_dark: true,
                },
                text,
            );
            rt.frame(0.);
            let elements = rt.elements();
            let column = elements
                .iter()
                .find(|e| e.key.as_deref() == Some("planner-content"))
                .unwrap();
            let answers: Vec<_> = elements
                .iter()
                .filter(|e| e.text.as_deref().is_some_and(|t| t.contains(&answer)))
                .collect();
            assert!(!answers.is_empty(), "{slide:?}");
            for value in answers {
                assert!(
                    value.rect[3] > 40.,
                    "{slide:?}: saved answer must wrap: {:?}",
                    value.rect
                );
                assert!(
                    value.rect[0] + value.rect[2] <= column.rect[0] + column.rect[2] + 1.,
                    "{slide:?}: answer overflows the column"
                );
            }
        }
    }
}

fn fixture(slide: Slide) -> Planner {
    let mut ledger = Ledger::default();
    let week = model::Week {
        date: if matches!(slide, Slide::Progress | Slide::Goal | Slide::Focus) {
            20_770
        } else {
            20_777
        },
        reflection: "Small morning steps kept scope tight and helped me finish".into(),
        goal: "Build a calm and dependable voice workspace".into(),
        remaining: "Finish the planner and verify the experience".into(),
        focus: "Ship the smallest useful daily planning flow".into(),
        success: "Complete morning planning in under three minutes".into(),
    };
    ledger.weeks.push(week.clone());
    ledger.week_draft = Some(week);
    ledger.days.push(model::Day {
        date: 20_774,
        deliverable: "Draft the planner ledger".into(),
        committed: true,
        delivered: Some(true),
        steps: vec![model::Step {
            text: "Write schema".into(),
            done: true,
        }],
        ..Default::default()
    });
    ledger.days.push(model::Day {
        date: 20_775,
        deliverable: "Prototype daily slides".into(),
        committed: true,
        delivered: Some(false),
        ..Default::default()
    });
    ledger.days.push(model::Day {
        date: 20_776,
        problem: "Make saved plans survive an app restart".into(),
        deliverable: "A working local ledger with durable saves".into(),
        criteria: "Drafts and bypass timers survive a restart".into(),
        committed: true,
        delivered: Some(slide != Slide::Yesterday),
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
        problem: "Make the daily planning overlay feel effortless".into(),
        deliverable: "A clean morning planning flow with checkable Option tasks".into(),
        criteria: "Every slide is scannable and Option clicks persist immediately".into(),
        committed: slide == Slide::DayView,
        breakdown_started: true,
        carry_over: if slide == Slide::Breakdown {
            vec!["Test an interrupted write".into()]
        } else {
            Vec::new()
        },
        steps: vec![
            model::Step {
                text: "Build the daily slides".into(),
                done: true,
            },
            model::Step {
                text: "Make Option task cards checkable".into(),
                done: false,
            },
            model::Step {
                text: "Review light and dark screenshots".into(),
                done: false,
            },
        ],
        ..Default::default()
    });
    if slide == Slide::Picture {
        ledger.weeks.last_mut().unwrap().goal = "Build a calm and dependable voice workspace that makes room for deliberate planning, keeps the larger goals visible, and turns each morning's focus into small steps with a clear finish.".into();
    }
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
            ("goal", Slide::Goal),
            ("focus", Slide::Focus),
            ("picture", Slide::Picture),
            ("yesterday", Slide::Yesterday),
            ("today", Slide::Today),
            ("outcome", Slide::Outcome),
            ("breakdown", Slide::Breakdown),
            ("day-view", Slide::DayView),
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
