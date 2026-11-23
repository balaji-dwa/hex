use rune::{Cursor, El, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, TextInput};
use rune_theme_opencode::icons;

use super::{
    EnterAction, check_row, copy, icon_action, inset_card, section_label, success_mark_colors,
    textarea_field,
};
use crate::rune_ui::planner::{
    Paint, Planner,
    model::{self, DayOutcome},
};

fn seven_day_strip(planner: &Planner, p: Paint) -> El {
    let days = planner.ledger.recent_seven_days(planner.date);
    let (ok_fill, ok_border, ok_check) = success_mark_colors(p);
    let mut dots = row().key("seven-day-strip").gap(10.).center_y();
    for (day, outcome) in days {
        let status_label = match outcome {
            DayOutcome::Delivered => "Delivered",
            DayOutcome::NotYet => "Not yet",
            DayOutcome::Planned => "Planned",
            DayOutcome::Today => "Today",
            DayOutcome::Skipped => "Skipped",
        };
        let mut dot = el().size(20., 20.).center().radius(999.);
        dot = match outcome {
            DayOutcome::Delivered => dot
                .fill(ok_fill)
                .border(1., ok_border)
                .child(icon(&icons::CHECK, 11.).color(ok_check)),
            DayOutcome::NotYet => dot
                .fill(p.k.c.warning.bg)
                .border(1.5, p.k.c.warning.fg)
                .child(el().size(6., 6.).radius(999.).fill(p.k.c.warning.fg)),
            DayOutcome::Planned => dot
                .fill(p.k.c.layer_02)
                .border(1.5, p.k.c.border_strong)
                .child(el().size(5., 5.).radius(999.).fill(p.text())),
            DayOutcome::Today => dot.fill(p.k.c.bg_base).border(1.5, p.text()),
            DayOutcome::Skipped => dot.fill(p.k.c.bg_base).border(1.5, p.k.c.border_strong),
        };
        dots = dots.child(
            col()
                .key(format!("day-dot-{day}"))
                .label(format!("{}: {status_label}", model::human_date_label(day)))
                .items_center()
                .gap(4.)
                .child(dot)
                .child(text(model::short_weekday(day)).font(p.k.ty.small).color(
                    if outcome == DayOutcome::Today {
                        p.text()
                    } else {
                        p.muted()
                    },
                )),
        );
    }
    dots
}

fn legend_chip(dot: El, label: &str, p: Paint) -> El {
    row()
        .gap(5.)
        .center_y()
        .child(dot)
        .child(text(label).font(p.k.ty.small).color(p.muted()))
}

fn recent_rhythm_card(planner: &Planner, p: Paint) -> El {
    let start = planner.date - 6;
    let days: Vec<_> = planner
        .ledger
        .days
        .iter()
        .filter(|day| day.date >= start && day.date < planner.date && day.committed)
        .collect();
    let delivered = days
        .iter()
        .filter(|day| day.delivered == Some(true))
        .count();
    let done = days
        .iter()
        .flat_map(|day| &day.steps)
        .filter(|step| step.done)
        .count();
    let stats_copy = if days.is_empty() {
        "Fresh start — no daily plans logged in the last 7 days.".into()
    } else {
        format!(
            "{} delivered · {} checked off · {} planned",
            model::plural(delivered, "day", "days"),
            model::plural(done, "step", "steps"),
            model::plural(days.len(), "day", "days"),
        )
    };
    let (ok_fill, _, _) = success_mark_colors(p);
    let legend = row()
        .key("seven-day-legend")
        .w_full()
        .gap(14.)
        .center_y()
        .child(legend_chip(
            el().size(8., 8.).radius(999.).fill(ok_fill),
            "Delivered",
            p,
        ))
        .child(legend_chip(
            el().size(8., 8.)
                .radius(999.)
                .fill(p.k.c.warning.bg)
                .border(1.5, p.k.c.warning.fg),
            "Not yet",
            p,
        ))
        .child(legend_chip(
            el().size(8., 8.)
                .radius(999.)
                .fill(p.k.c.layer_02)
                .border(1.5, p.k.c.border_strong),
            "Planned",
            p,
        ))
        .child(legend_chip(
            el().size(8., 8.)
                .radius(999.)
                .fill(p.k.c.bg_base)
                .border(1.5, p.k.c.border_strong),
            "Skipped",
            p,
        ));

    inset_card("week-rhythm", p)
        .child(
            row()
                .w_full()
                .center_y()
                .gap(12.)
                .child(
                    col()
                        .grow(1.)
                        .shrink()
                        .gap(3.)
                        .child(section_label("Last 7 days", p))
                        .child(
                            text(stats_copy)
                                .key("progress")
                                .font(p.k.ty.ui)
                                .color(p.text())
                                .wrap_anywhere(),
                        ),
                )
                .child(seven_day_strip(planner, p)),
        )
        .child(legend)
}

pub(super) fn picture(planner: &Planner, p: Paint) -> El {
    let mut content = col().key("summary").w_full().gap(14.);
    content = content.child(recent_rhythm_card(planner, p));
    if let Some(week) = planner.ledger.weeks.last() {
        let mut card = inset_card("week-anchor", p);
        let items = [
            ("goal", "Overarching goal", &week.goal),
            ("focus", "Weekly focus", &week.focus),
            ("success", "What makes a good week", &week.success),
            ("remaining", "Still to do", &week.remaining),
        ];
        for (idx, (key, label, value)) in items.into_iter().enumerate() {
            if idx > 0 {
                card = card.child(el().w_full().h(p.hair).fill(p.k.c.border_muted));
            }
            card = card.child(
                col()
                    .key(key)
                    .w_full()
                    .gap(3.)
                    .child(section_label(label, p))
                    .child(copy("value", value.clone(), p)),
            );
        }
        content = content.child(card);
    }
    content
}

pub(super) fn weekly_review(planner: &Planner, p: Paint) -> El {
    let (ok_fill, ok_border, ok_check) = success_mark_colors(p);
    let mut content = col().key("summary").w_full().gap(10.);
    content = content.child(recent_rhythm_card(planner, p).px(16.).py(12.).gap(8.));
    if let Some(week) = planner.ledger.weeks.last() {
        let days: Vec<_> = planner
            .ledger
            .days
            .iter()
            .filter(|day| day.date >= week.date && day.date < planner.date && day.committed)
            .collect();
        let mut card = inset_card("week-anchor", p).px(16.).py(12.).gap(8.).child(
            col()
                .key("focus")
                .w_full()
                .gap(2.)
                .child(section_label("Last week’s focus", p))
                .child(copy("value", &week.focus, p)),
        );
        if !days.is_empty() {
            card = card
                .child(el().w_full().h(p.hair).fill(p.k.c.border_muted))
                .child(section_label("Deliverables", p));
            for day in days {
                let met = day.delivered == Some(true);
                let marker = if met {
                    el().size(16., 16.)
                        .mt(2.)
                        .center()
                        .radius(999.)
                        .fill(ok_fill)
                        .border(1., ok_border)
                        .child(icon(&icons::CHECK, 10.).color(ok_check))
                } else {
                    el().size(16., 16.)
                        .mt(2.)
                        .center()
                        .radius(999.)
                        .fill(p.k.c.warning.bg)
                        .border(1.5, p.k.c.warning.fg)
                        .child(el().size(4., 4.).radius(999.).fill(p.k.c.warning.fg))
                };
                card = card.child(
                    row()
                        .key(format!("deliverable-{}", day.date))
                        .w_full()
                        .gap(8.)
                        .child(marker)
                        .child(copy("value", &day.deliverable, p).grow(1.).shrink()),
                );
            }
        }
        content = content.child(card);
    }
    content.child(textarea_field(
        planner,
        4,
        Some("What worked? What needs to change?"),
        None,
        "e.g. Smaller morning steps helped me ship; protect the first two hours from Slack.",
        2,
        4,
        EnterAction::Advance,
        p,
    ))
}

pub(super) fn goal_slide(planner: &Planner, p: Paint) -> El {
    col()
        .w_full()
        .gap(16.)
        .child(textarea_field(
            planner,
            5,
            Some("What are you working toward?"),
            Some("Your overarching goal across the coming weeks."),
            "e.g. Build a calm, dependable voice workspace I use every day",
            2,
            5,
            EnterAction::FocusField("answer-6"),
            p,
        ))
        .child(textarea_field(
            planner,
            6,
            Some("What still needs to happen?"),
            Some("The major remaining milestones between here and that goal."),
            "e.g. Polish the morning planner flow and verify everyday reliability",
            2,
            5,
            EnterAction::Advance,
            p,
        ))
}

pub(super) fn focus_slide(planner: &Planner, p: Paint) -> El {
    col()
        .w_full()
        .gap(16.)
        .child(textarea_field(
            planner,
            7,
            Some("What will you focus on this week?"),
            Some("One main theme that guides your daily choices this week."),
            "e.g. Ship a calm, keyboard-first daily planning ritual",
            2,
            5,
            EnterAction::FocusField("answer-8"),
            p,
        ))
        .child(textarea_field(
            planner,
            8,
            Some("What would make this a good week?"),
            Some("The clear outcome you’ll look back on seven days from now."),
            "e.g. Use the planner every morning and check off steps with Option",
            2,
            5,
            EnterAction::Advance,
            p,
        ))
}

fn choice_card(
    key: &str,
    title: &str,
    detail: &str,
    selected: bool,
    positive: bool,
    p: Paint,
    press: impl Fn(&mut Planner, &mut Tx) + 'static,
) -> El {
    let (fill, border_w, border_c, title_c) = if selected {
        if positive {
            (p.k.c.success.bg, 1.5, p.k.c.success.border, p.success())
        } else {
            (
                p.k.c.warning.bg,
                1.5,
                p.k.c.warning.border,
                super::warning_text(p),
            )
        }
    } else {
        (p.k.c.bg_base, 1., p.k.c.border_base, p.text())
    };
    let card = col()
        .key(key)
        .role("radio")
        .label(title)
        .checked(selected)
        .grow(1.)
        .shrink()
        .px(14.)
        .py(12.)
        .gap(3.)
        .radius(p.k.radius.md)
        .fill(fill)
        .border(border_w, border_c)
        .cursor(Cursor::Pointer)
        .focusable(true)
        .on_press_tx(press)
        .child(text(title).font(p.k.ty.ui_medium).color(title_c))
        .child(text(detail).font(p.k.ty.small).color(p.muted()).wrap());
    p.k.interactive(card)
}

pub(super) fn yesterday(planner: &Planner, p: Paint) -> El {
    let mut content = col().key("yesterday").w_full().gap(12.);
    let Some(previous) = planner.ledger.previous(planner.date) else {
        return content.child(copy("first-day", "No saved plan yet.", p));
    };
    let date = previous.date;
    let plan_heading = if planner.date - date == 1 {
        format!("Yesterday · {}", model::human_date_label(date))
    } else {
        format!("Last plan · {}", model::human_date_label(date))
    };
    content = content.child(
        inset_card("previous-plan", p)
            .px(16.)
            .py(12.)
            .gap(8.)
            .child(
                text(plan_heading)
                    .key("previous-plan-date")
                    .font(p.k.ty.ui_medium)
                    .color(p.text()),
            )
            .child(el().w_full().h(p.hair).fill(p.k.c.border_muted))
            .child(
                row()
                    .w_full()
                    .gap(16.)
                    .child(
                        col()
                            .grow(1.)
                            .shrink()
                            .gap(3.)
                            .child(section_label("Deliverable", p))
                            .child(copy("previous-deliverable", &previous.deliverable, p)),
                    )
                    .child(
                        col()
                            .grow(1.)
                            .shrink()
                            .gap(3.)
                            .child(section_label("Done when", p))
                            .child(copy("previous-criteria", &previous.criteria, p)),
                    ),
            ),
    );

    content = content.child(
        col()
            .w_full()
            .gap(8.)
            .child(
                text("Did you meet your criteria?")
                    .font(p.k.ty.ui_medium)
                    .color(p.text()),
            )
            .child(
                row()
                    .w_full()
                    .gap(10.)
                    .child(choice_card(
                        "delivered-yes",
                        "Yes, done",
                        "Finished what I committed to (marks steps done)",
                        previous.delivered == Some(true),
                        true,
                        p,
                        move |state, tx| state.mark_yesterday_delivered(date, true, tx),
                    ))
                    .child(choice_card(
                        "delivered-no",
                        "Not yet",
                        "Still in progress or scope shifted",
                        previous.delivered == Some(false),
                        false,
                        p,
                        move |state, tx| state.mark_yesterday_delivered(date, false, tx),
                    )),
            ),
    );

    if previous.delivered == Some(false) {
        content = content.child(
            col()
                .key("blocker-note")
                .w_full()
                .gap(5.)
                .child(section_label("What got in the way? (optional)", p))
                .child(
                    TextInput::new(&planner.fields[0])
                        .width(None)
                        .label("What got in the way")
                        .placeholder("e.g. Underestimated testing, pulled into an urgent review…")
                        .on_edit(|state: &mut Planner| &mut state.fields[0])
                        .on_change(|state: &mut Planner, tx| state.edited(tx))
                        .on_submit(|state: &mut Planner, tx| state.advance(tx))
                        .build(p.k)
                        .key("blocker-input"),
                ),
        );
    }

    if !previous.steps.is_empty() {
        let mut list = col().key("yesterday-tasks").w_full().gap(6.).child(
            row()
                .w_full()
                .center_y()
                .child(
                    text("Steps from your last plan")
                        .font(p.k.ty.ui_medium)
                        .color(p.text())
                        .grow(1.),
                )
                .child(
                    text("Unchecked steps will carry over to “Break it into small steps”")
                        .font(p.k.ty.small)
                        .color(p.muted()),
                ),
        );
        for (index, step) in previous.steps.iter().enumerate() {
            list = list.child(
                check_row(
                    format!("previous-step-{index}"),
                    &step.text,
                    step.done,
                    p,
                    move |state, tx| state.toggle_yesterday_step(date, index, tx),
                )
                .py(8.),
            );
        }
        content = content.child(list);
    }

    content
}

fn context_strip(items: &[(&str, &str, &str)], p: Paint) -> El {
    let mut strip = col()
        .key("context-strip")
        .w_full()
        .px(16.)
        .py(12.)
        .gap(8.)
        .radius(p.k.radius.lg)
        .fill(p.k.c.bg_base)
        .border(p.hair, p.k.c.border_base);
    for (idx, (key, label, value)) in items.iter().enumerate() {
        if value.trim().is_empty() {
            continue;
        }
        if idx > 0 {
            strip = strip.child(el().w_full().h(p.hair).fill(p.k.c.border_muted));
        }
        strip = strip.child(
            col()
                .w_full()
                .gap(2.)
                .child(section_label(label, p))
                .child(copy(key, *value, p)),
        );
    }
    strip
}

pub(super) fn today_slide(planner: &Planner, p: Paint) -> El {
    let mut body = col().w_full().gap(16.);
    if let Some(week) = planner.ledger.weeks.last()
        && !week.focus.trim().is_empty()
    {
        body = body.child(context_strip(
            &[("weekly-focus-ref", "This week’s focus", &week.focus)],
            p,
        ));
    }
    body.child(textarea_field(
        planner,
        1,
        None,
        Some("One clear sentence is plenty."),
        "e.g. Ship the checkable Option todo list and verify dark and light mode",
        2,
        6,
        EnterAction::Advance,
        p,
    ))
}

pub(super) fn outcome_slide(planner: &Planner, p: Paint) -> El {
    col()
        .w_full()
        .gap(16.)
        .child(context_strip(
            &[("pinned-problem", "Today’s focus", &planner.fields[1].text)],
            p,
        ))
        .child(textarea_field(
            planner,
            2,
            Some("What will you deliver?"),
            Some("The concrete output you want finished by the end of the day."),
            "e.g. A merged PR and verified local macOS build with checkable Option tasks",
            2,
            5,
            EnterAction::FocusField("answer-3"),
            p,
        ))
        .child(textarea_field(
            planner,
            3,
            Some("How will you know it’s done?"),
            Some("A crisp finish line so you don’t keep drifting or gold-plating."),
            "e.g. Holding Option shows today’s steps, clicking checks them off, and all tests pass",
            2,
            5,
            EnterAction::Advance,
            p,
        ))
}

fn task_entry(planner: &Planner, p: Paint) -> El {
    let pending_invalid = planner.error.is_some()
        && (!planner.fields[9].text.trim().is_empty() || planner.steps.is_empty());
    let next_num = format!("{:02}", planner.steps.len() + 1);
    row()
        .key("task-entry")
        .w_full()
        .gap(8.)
        .center_y()
        .child(
            text(next_num)
                .font(p.k.ty.small_medium.features("'tnum' 1"))
                .color(p.faint())
                .w(22.),
        )
        .child(
            TextInput::new(&planner.fields[9])
                .width(None)
                .label("Add a step")
                .placeholder(if planner.steps.is_empty() {
                    "Add a concrete step and press Enter…"
                } else {
                    "Add another step, or press Enter to commit…"
                })
                .invalid(pending_invalid)
                .on_edit(|state: &mut Planner| &mut state.fields[9])
                .on_change(|state: &mut Planner, tx| state.edited(tx))
                .on_submit(|state: &mut Planner, tx| {
                    if state.fields[9].text.trim().is_empty() && !state.steps.is_empty() {
                        state.advance(tx);
                    } else {
                        state.add_step(tx);
                    }
                })
                .build(p.k)
                .key("new-step")
                .grow(1.)
                .shrink(),
        )
        .child(
            Button::new("Add")
                .variant(ButtonVariant::Neutral)
                .control_size(ControlSize::Regular)
                .on_press_tx(|state: &mut Planner, tx| state.add_step(tx))
                .build(p.k)
                .key("add-step"),
        )
}

pub(super) fn breakdown(planner: &Planner, p: Paint) -> El {
    let mut body = col().w_full().gap(16.);
    body = body.child(context_strip(
        &[
            ("pinned-problem", "Today’s focus", &planner.fields[1].text),
            ("pinned-deliverable", "Deliverable", &planner.fields[2].text),
        ],
        p,
    ));

    let carry_over = planner
        .ledger
        .day_ref(planner.date)
        .map(|day| day.carry_over.clone())
        .unwrap_or_default();
    if !carry_over.is_empty() {
        let mut header = row().w_full().center_y().gap(8.).child(
            text(format!("From yesterday ({} unfinished)", carry_over.len()))
                .font(p.k.ty.small_medium)
                .color(p.muted())
                .grow(1.),
        );
        if carry_over.len() > 1 {
            header = header
                .child(
                    Button::new("Keep all")
                        .variant(ButtonVariant::Neutral)
                        .control_size(ControlSize::Small)
                        .on_press_tx(|state: &mut Planner, tx| state.keep_all_carry(tx))
                        .build(p.k)
                        .key("keep-all-carry"),
                )
                .child(
                    Button::new("Drop all")
                        .variant(ButtonVariant::Outline)
                        .control_size(ControlSize::Small)
                        .on_press_tx(|state: &mut Planner, tx| state.drop_all_carry(tx))
                        .build(p.k)
                        .key("drop-all-carry"),
                );
        }
        let mut box_card = inset_card("carry-over-triage", p).child(header);
        for (index, item) in carry_over.iter().enumerate() {
            box_card = box_card.child(
                row()
                    .key(format!("carry-item-{index}"))
                    .w_full()
                    .center_y()
                    .gap(8.)
                    .child(copy("carry-text", item, p).grow(1.).shrink())
                    .child(
                        Button::new("Keep")
                            .variant(ButtonVariant::Neutral)
                            .control_size(ControlSize::Small)
                            .on_press_tx(move |state: &mut Planner, tx| state.keep_carry(index, tx))
                            .build(p.k)
                            .key(format!("keep-carry-{index}")),
                    )
                    .child(
                        Button::new("× Drop")
                            .variant(ButtonVariant::Outline)
                            .control_size(ControlSize::Small)
                            .on_press_tx(move |state: &mut Planner, tx| state.drop_carry(index, tx))
                            .build(p.k)
                            .key(format!("drop-carry-{index}")),
                    ),
            );
        }
        body = body.child(box_card);
    }

    let mut list = col().key("steps").w_full().gap(10.);
    for (index, step) in planner.steps.iter().enumerate() {
        let step_invalid = planner.error.is_some() && !model::task(&step.text);
        list = list.child(
            row()
                .key(format!("step-{index}"))
                .w_full()
                .center_y()
                .gap(8.)
                .child(
                    text(format!("{:02}", index + 1))
                        .font(p.k.ty.small_medium.features("'tnum' 1"))
                        .color(p.muted())
                        .w(22.),
                )
                .child(
                    TextInput::new(step)
                        .width(None)
                        .label("Plan step · at least three characters")
                        .invalid(step_invalid)
                        .on_edit(rune::edit_access(move |state: &mut Planner| {
                            &mut state.steps[index]
                        }))
                        .on_change(|state: &mut Planner, tx| state.edited(tx))
                        // Editing a step should never accidentally skip the slide.
                        .on_submit(|_: &mut Planner, tx| tx.focus("new-step", false))
                        .build(p.k)
                        .key(format!("step-field-{index}"))
                        .grow(1.)
                        .shrink(),
                )
                .child(icon_action(
                    &format!("remove-step-{index}"),
                    "Remove item",
                    &icons::CLOSE,
                    p,
                    move |state, tx| {
                        state.steps.remove(index);
                        state.ledger.today(state.date).steps.remove(index);
                        state.edited(tx);
                    },
                )),
        );
    }
    list = list.child(task_entry(planner, p));

    if planner.steps.len() > model::RECOMMENDED_MAX_STEPS {
        list = list.child(
            row()
                .key("step-cap-nudge")
                .w_full()
                .px(12.)
                .py(9.)
                .radius(p.k.radius.md)
                .fill(p.k.c.warning.bg)
                .border(p.hair, p.k.c.warning.border)
                .child(
                    text(format!(
                        "{} steps is a full day — consider trimming to 3–5 essentials so you don’t get overwhelmed.",
                        planner.steps.len()
                    ))
                    .font(p.k.ty.small)
                    .color(super::warning_text(p))
                    .wrap(),
                ),
        );
    }

    body.child(list)
}

pub(super) fn day_view(planner: &Planner, p: Paint) -> El {
    let day = planner.ledger.day_ref(planner.date);
    let steps = day.map(|d| d.steps.as_slice()).unwrap_or(&[]);
    let total = steps.len();
    let done = steps.iter().filter(|s| s.done).count();
    let all_done = total > 0 && done == total;

    let mut body = col().key("day-view").w_full().gap(16.);

    body = body.child(
        inset_card("day-commitment", p)
            .child(
                col()
                    .w_full()
                    .gap(3.)
                    .child(section_label("Focus", p))
                    .child(copy("day-focus", &planner.fields[1].text, p)),
            )
            .child(el().w_full().h(p.hair).fill(p.k.c.border_muted))
            .child(
                col()
                    .w_full()
                    .gap(3.)
                    .child(section_label("Deliverable", p))
                    .child(copy("day-deliverable", &planner.fields[2].text, p)),
            )
            .child(el().w_full().h(p.hair).fill(p.k.c.border_muted))
            .child(
                col()
                    .w_full()
                    .gap(3.)
                    .child(section_label("Done when", p))
                    .child(copy("day-criteria", &planner.fields[3].text, p)),
            ),
    );

    let mut step_section = col().key("day-steps").w_full().gap(10.).child(
        row()
            .w_full()
            .center_y()
            .child(
                text("Today’s steps")
                    .font(p.k.ty.ui_medium)
                    .color(p.text())
                    .grow(1.),
            )
            .child(
                text(if all_done {
                    "All done".into()
                } else {
                    format!("{done} of {total} done")
                })
                .font(p.k.ty.small_medium.features("'tnum' 1"))
                .color(if all_done { p.success() } else { p.muted() }),
            ),
    );

    if total > 0 {
        let ratio = (done as f32 / total as f32).clamp(0.0, 1.0);
        step_section = step_section.child(
            row()
                .key("day-progress")
                .w_full()
                .h(4.)
                .radius(999.)
                .fill(p.k.c.layer_02)
                .clip_soft(0.)
                .child(el().h_full().w_frac(ratio).radius(999.).fill(if all_done {
                    p.k.c.success.fg
                } else {
                    p.text()
                })),
        );
    }

    for (index, step) in steps.iter().enumerate() {
        step_section = step_section.child(check_row(
            format!("today-step-{index}"),
            &step.text,
            step.done,
            p,
            move |state, _| {
                state.toggle_today_step(index);
            },
        ));
    }

    body.child(step_section)
}
