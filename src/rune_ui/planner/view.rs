use rune::{Cx, El, ScrollSpec, TextStyle, Tx, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, Checkbox, ControlSize, IconButton, IconButtonVariant, TextArea,
    TextInput,
};
use rune_theme_opencode::icons;

use super::{Paint, Planner, Slide, model};

fn button(
    key: &str,
    label: &str,
    p: Paint,
    primary: bool,
    press: impl Fn(&mut Planner, &mut Tx) + 'static,
) -> El {
    Button::new(label)
        .variant(if primary {
            ButtonVariant::Contrast
        } else {
            ButtonVariant::Ghost
        })
        .on_press_tx(press)
        .build(p.k)
        .key(key)
}

fn action(
    key: &str,
    label: &str,
    glyph: &'static rune::Icon,
    p: Paint,
    press: impl Fn(&mut Planner, &mut Tx) + 'static,
) -> El {
    IconButton::new(glyph)
        .label(label)
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .on_press_tx(press)
        .build(p.k)
        .key(key)
}

fn copy(key: &str, value: impl Into<String>, p: Paint) -> El {
    text(value.into()).key(key).font(p.k.ty.ui).color(p.text())
}

fn field(planner: &Planner, index: usize, label: &str, p: Paint) -> El {
    col().key(format!("question-{index}")).gap(12.).child(
        TextArea::new(&planner.fields[index])
            .label(label)
            .placeholder("Write here…")
            .rows(10)
            .width(None)
            .on_edit(rune::edit_access(move |state: &mut Planner| {
                &mut state.fields[index]
            }))
            .on_change(|state: &mut Planner, tx| state.edited(tx))
            .on_key(|state: &mut Planner, event, tx| {
                if event.key == rune::Key::Enter && !event.mods.shift {
                    state.advance(tx);
                    true
                } else {
                    false
                }
            })
            .build(p.k)
            .key(format!("answer-{index}")),
    )
}

fn task_entry(planner: &Planner, p: Paint) -> El {
    row()
        .key("task-entry")
        .gap(8.)
        .center_y()
        .child(
            TextInput::new(&planner.fields[9])
                .width(None)
                .label("Add a step")
                .placeholder("Add an item…")
                .on_edit(|state: &mut Planner| &mut state.fields[9])
                .on_change(|state: &mut Planner, tx| state.edited(tx))
                .on_submit(|state: &mut Planner, tx| state.add_step(tx))
                .build(p.k)
                .key("new-step")
                .grow(1.)
                .shrink(),
        )
        .child(action(
            "add-step",
            "Add item · at least three characters",
            &icons::PLUS,
            p,
            |state, tx| state.add_step(tx),
        ))
}

fn check(
    key: String,
    label: &str,
    done: bool,
    p: Paint,
    press: impl Fn(&mut Planner, &mut Tx) + 'static,
) -> El {
    Checkbox::new(done)
        .label(label)
        .on_toggle_tx(press)
        .build(p.k)
        .key(key)
        .py(4.)
}

fn summary(planner: &Planner, p: Paint, weekly: bool) -> El {
    let mut content = col().key("summary").gap(16.);
    if let Some(week) = planner.ledger.weeks.last() {
        for (key, label, value) in [
            ("goal", "Goal", &week.goal),
            ("focus", "This week", &week.focus),
            ("success", "A good week", &week.success),
            ("remaining", "Still to do", &week.remaining),
        ] {
            content = content.child(
                col()
                    .key(key)
                    .gap(5.)
                    .child(text(label).font(p.k.ty.small_medium).color(p.muted()))
                    .child(copy("value", value.clone(), p)),
            );
        }
        let days: Vec<_> = planner
            .ledger
            .days
            .iter()
            .filter(|day| day.date >= week.date && day.date < planner.date && day.committed)
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
        content = content.child(copy("progress", format!("{delivered} deliverables completed · {done} steps checked off · {} plans committed", days.len()), p));
        if weekly {
            for day in days {
                content = content.child(copy(
                    &format!("deliverable-{}", day.date),
                    format!(
                        "{}  {}",
                        if day.delivered == Some(true) {
                            "✓"
                        } else {
                            "○"
                        },
                        day.deliverable
                    ),
                    p,
                ));
            }
        }
    }
    content
}

fn yesterday(planner: &Planner, p: Paint) -> El {
    let mut content = col().key("yesterday").gap(16.);
    if let Some(previous) = planner.ledger.previous(planner.date) {
        let date = previous.date;
        content = content
            .child(
                text(if planner.date - date == 1 {
                    "Yesterday's plan".into()
                } else {
                    format!("Last plan · {}", model::date_label(date))
                })
                .font(p.k.ty.small_medium)
                .color(p.muted()),
            )
            .child(copy("previous-deliverable", &previous.deliverable, p))
            .child(copy("previous-criteria", &previous.criteria, p))
            .child(
                row()
                    .gap(8.)
                    .children([(true, "Yes"), (false, "Not yet")].map(|(answer, label)| {
                        button(
                            if answer {
                                "delivered-yes"
                            } else {
                                "delivered-no"
                            },
                            label,
                            p,
                            previous.delivered == Some(answer),
                            move |state, tx| {
                                state.ledger.today(date).delivered = Some(answer);
                                state.advance(tx);
                            },
                        )
                    })),
            );
    } else {
        content = content.child(copy("first-day", "No saved plan yet.", p));
    }
    content
}

fn yesterday_tasks(planner: &Planner, p: Paint) -> El {
    let mut content = col().key("yesterday-tasks").gap(16.);
    if let Some(previous) = planner.ledger.previous(planner.date) {
        let date = previous.date;
        for (index, step) in previous.steps.iter().enumerate() {
            content = content.child(check(
                format!("previous-step-{index}"),
                &step.text,
                step.done,
                p,
                move |state, tx| {
                    let step = &mut state.ledger.today(date).steps[index];
                    step.done = !step.done;
                    state.edited(tx);
                },
            ));
        }
        if previous.steps.is_empty() {
            content = content.child(copy("no-steps", "No steps saved.", p));
        }
    }
    content
}

fn breakdown(planner: &Planner, p: Paint) -> El {
    let mut list = col().key("steps").gap(10.);
    for (index, step) in planner.steps.iter().enumerate() {
        list = list.child(
            row()
                .key(format!("step-{index}"))
                .center_y()
                .gap(8.)
                .child(
                    TextInput::new(step)
                        .width(None)
                        .label("Plan step · at least three characters")
                        .on_edit(rune::edit_access(move |state: &mut Planner| {
                            &mut state.steps[index]
                        }))
                        .on_change(|state: &mut Planner, tx| state.edited(tx))
                        .on_submit(|state: &mut Planner, tx| state.advance(tx))
                        .build(p.k)
                        .key(format!("step-field-{index}"))
                        .grow(1.)
                        .shrink(),
                )
                .child(action(
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
    list.child(task_entry(planner, p))
}

pub(super) fn view(planner: &Planner, cx: &mut Cx<Planner>) -> El {
    let p = Paint::new(cx.system_dark(), cx.env().scale);
    let slides = planner.slides();
    let index = slides
        .iter()
        .position(|slide| *slide == planner.slide)
        .unwrap_or(0);
    let today_done = planner
        .ledger
        .days
        .iter()
        .any(|day| day.date == planner.date && day.committed);
    let mut content = col()
        .key("planner-content")
        .focusable(true)
        .w_full()
        .max_w(720.)
        .gap(24.)
        .child(
            row()
                .key("slide-navigation")
                .gap(6.)
                .center_y()
                .child(
                    action(
                        "planner-back",
                        "Previous slide",
                        &icons::ARROW_LEFT,
                        p,
                        |state, tx| state.back(tx),
                    )
                    .disabled(index == 0),
                )
                .child(action(
                    "planner-next",
                    if planner.slide == Slide::Criteria {
                        "Commit to today"
                    } else {
                        "Next slide"
                    },
                    &icons::ARROW_RIGHT,
                    p,
                    |state, tx| state.advance(tx),
                ))
                .child(el().w(8.))
                .child(
                    text(if planner.ledger.week_due(planner.date) {
                        "Weekly"
                    } else {
                        "Daily"
                    })
                    .font(p.k.ty.small_medium)
                    .color(p.muted()),
                ),
        )
        .child(
            text(planner.slide.title())
                .role("heading")
                .level(1)
                .font(TextStyle::new(32., 500.).lh(40.))
                .color(p.text()),
        );
    if matches!(
        planner.slide,
        Slide::Breakdown | Slide::Commit | Slide::Criteria
    ) {
        content = content.child(
            col()
                .gap(6.)
                .child(
                    text("Today's focus")
                        .font(p.k.ty.small_medium)
                        .color(p.muted()),
                )
                .child(copy("pinned-problem", &planner.fields[1].text, p)),
        );
    }
    let body = match planner.slide {
        Slide::Progress => summary(planner, p, true),
        Slide::Reflection => field(planner, 4, "What worked? What needs to change?", p),
        Slide::Goal => field(planner, 5, "What are you working toward?", p),
        Slide::Remaining => field(planner, 6, "What still needs to happen?", p),
        Slide::Focus => field(planner, 7, "What will you focus on this week?", p),
        Slide::Success => field(planner, 8, "What would make this a good week?", p),
        Slide::Picture => summary(planner, p, false),
        Slide::Yesterday => yesterday(planner, p),
        Slide::YesterdayTasks => yesterday_tasks(planner, p),
        Slide::Today => field(planner, 1, "What are you tackling?", p),
        Slide::Breakdown => breakdown(planner, p),
        Slide::Commit => field(planner, 2, "What will you deliver?", p),
        Slide::Criteria => field(planner, 3, "How will you know it’s done?", p),
    };
    content = content.child(body);
    if let Some(error) = &planner.error {
        content = content.child(
            text(error)
                .key("planner-error")
                .role("alert")
                .font(p.k.ty.ui)
                .color(p.danger()),
        );
    }
    if let Some(note) = &planner.login_note {
        content = content.child(
            text(note)
                .key("planner-login-note")
                .font(p.k.ty.small)
                .color(p.muted()),
        );
    }
    let footer = row()
        .key("planner-footer")
        .w_full()
        .px(24.)
        .py(16.)
        .center_y()
        .child(
            text(if today_done && !planner.ledger.week_due(planner.date) {
                "← → slides · Enter next · Esc close"
            } else {
                "← → slides · Enter next · Esc bypass 5 min"
            })
            .font(p.k.ty.small)
            .color(p.muted())
            .shrink()
            .truncate(),
        )
        .child(el().grow(1.))
        .child(
            Button::new(if today_done && !planner.ledger.week_due(planner.date) {
                "Close planner"
            } else {
                "Bypass · 5 min"
            })
            .variant(ButtonVariant::Ghost)
            .control_size(ControlSize::Small)
            .on_press(|state: &mut Planner| state.dismiss(crate::history::now_ms() / 1_000))
            .build(p.k)
            .key("planner-bypass")
            .opacity(0.75),
        );
    col()
        .key("planner")
        .role("dialog")
        .label("Planner")
        .w_full()
        .h_full()
        .fill(p.k.c.bg_base)
        .child(
            col()
                .w_full()
                .grow(1.)
                .shrink()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(col().w_full().items_center().px(32.).py(48.).child(content)),
        )
        .child(footer)
}
