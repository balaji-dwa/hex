use rune::{Cursor, Cx, El, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, TextArea};
use rune_theme_opencode::icons;

use super::{Paint, Planner, Slide, model};

mod slides;

pub(super) fn copy(key: &str, value: impl Into<String>, p: Paint) -> El {
    text(value.into())
        .key(key)
        .font(p.k.ty.ui)
        .color(p.text())
        .wrap_anywhere()
}

pub(super) fn section_label(label: &str, p: Paint) -> El {
    text(label).font(p.k.ty.small_medium).color(p.muted())
}

pub(super) fn inset_card(key: &str, p: Paint) -> El {
    col()
        .key(key)
        .w_full()
        .padding(16.)
        .gap(10.)
        .radius(p.k.radius.lg)
        .fill(p.k.c.bg_base)
        .border(p.hair, p.k.c.border_base)
}

pub(super) fn icon_action(
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

pub(super) fn success_mark_colors(p: Paint) -> (rune::Color, rune::Color, rune::Color) {
    if p.k.dark {
        (p.k.c.success.fg, p.k.c.success.fg, p.k.c.bg_base)
    } else {
        (p.k.c.success.icon, p.k.c.success.icon, p.k.c.white)
    }
}

/// High-contrast warning text color on `warning.bg` in both light and dark themes.
pub(super) fn warning_text(p: Paint) -> rune::Color {
    if p.k.dark {
        p.warning()
    } else {
        p.warning().mix(p.text(), 0.45)
    }
}

/// Strikethrough spans per non-whitespace word so wrapped lines never overshoot trailing spaces.
pub(super) fn word_strikes(
    text: &str,
    color: rune::Color,
) -> Vec<(usize, usize, Option<rune::Color>)> {
    let mut out = Vec::new();
    let mut start = None;
    for (idx, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                out.push((s, idx, Some(color)));
            }
        } else if start.is_none() {
            start = Some(idx);
        }
    }
    if let Some(s) = start {
        out.push((s, text.len(), Some(color)));
    }
    out
}

pub(super) fn check_row(
    key: String,
    label: &str,
    done: bool,
    p: Paint,
    press: impl Fn(&mut Planner, &mut Tx) + 'static,
) -> El {
    let mut box_el = el()
        .key("checkbox-box")
        .size(18., 18.)
        .mt(1.)
        .center()
        .radius(5.)
        .a11y_hidden();
    if done {
        let (fill, border, check_color) = success_mark_colors(p);
        box_el = box_el
            .fill(fill)
            .border(1., border)
            .child(icon(&icons::CHECK, 12.).color(check_color));
    } else {
        box_el = box_el.fill(p.k.c.bg_base).border(1.5, p.k.c.border_strong);
    }
    let mut txt = text(label)
        .key("label")
        .font(p.k.ty.ui)
        .color(if done { p.muted() } else { p.text() })
        .wrap_anywhere()
        .grow(1.)
        .shrink();
    if done {
        txt = txt.strikes(word_strikes(label, p.muted()));
    }
    // Open rows stand slightly forward on the card; finished rows recede into the card surface.
    let (row_fill, row_border) = if done {
        (p.k.c.layer_01, p.k.c.border_muted)
    } else if p.k.dark {
        (p.k.c.layer_02, p.k.c.border_base)
    } else {
        (p.k.c.bg_base, p.k.c.border_base)
    };
    let row_el = row()
        .key(key)
        .role("checkbox")
        .label(label)
        .checked(done)
        .w_full()
        .gap(12.)
        .px(14.)
        .py(11.)
        .radius(p.k.radius.md)
        .fill(row_fill)
        .border(p.hair, row_border)
        .cursor(Cursor::Pointer)
        .focusable(true)
        .on_press_tx(press)
        .child(box_el)
        .child(txt);
    p.k.interactive(row_el)
}

#[derive(Clone, Copy)]
pub(super) enum EnterAction {
    Advance,
    FocusField(&'static str),
}

#[allow(clippy::too_many_arguments)]
pub(super) fn textarea_field(
    planner: &Planner,
    index: usize,
    title: Option<&str>,
    hint: Option<&str>,
    placeholder: &str,
    min_rows: usize,
    max_rows: usize,
    enter_action: EnterAction,
    p: Paint,
) -> El {
    let invalid = planner.error.is_some() && !model::answer(&planner.fields[index].text);
    let mut group = col().key(format!("question-{index}")).w_full().gap(6.);
    if let Some(title) = title {
        group = group.child(text(title).font(p.k.ty.ui_medium).color(p.text()));
    }
    let label = title.unwrap_or(placeholder);
    group = group.child(
        TextArea::new(&planner.fields[index])
            .label(label)
            .placeholder(placeholder)
            .auto_grow(min_rows, max_rows)
            .width(None)
            .invalid(invalid)
            .on_edit(rune::edit_access(move |state: &mut Planner| {
                &mut state.fields[index]
            }))
            .on_change(|state: &mut Planner, tx| state.edited(tx))
            .on_key(move |state: &mut Planner, event, tx| {
                if event.key == rune::Key::Enter {
                    if event.mods.cmd || event.mods.ctrl {
                        state.advance(tx);
                        return true;
                    }
                    if !event.mods.shift {
                        match enter_action {
                            EnterAction::Advance => state.advance(tx),
                            EnterAction::FocusField(target) => tx.focus(target, false),
                        }
                        return true;
                    }
                }
                false
            })
            .build(p.k)
            .key(format!("answer-{index}")),
    );
    if let Some(hint) = hint {
        group = group.child(text(hint).font(p.k.ty.small).color(p.muted()).wrap());
    }
    group
}

fn header_bar(planner: &Planner, all_slides: &[Slide], index: usize, p: Paint) -> El {
    let weekly = planner.ledger.week_due(planner.date);
    let committed = planner.slide == Slide::DayView;
    let editing = planner.edit_backup.is_some();
    let date_text = model::human_date_label(planner.date);

    let pill_label = if committed {
        "Committed"
    } else if editing {
        "Editing plan"
    } else if weekly {
        "Weekly review"
    } else {
        "Daily plan"
    };

    let pill = row()
        .key("flow-pill")
        .px(9.)
        .h(22.)
        .center()
        .radius(999.)
        .fill(if committed {
            p.k.c.success.bg
        } else {
            p.k.c.layer_02
        })
        .border(
            p.hair,
            if committed {
                p.k.c.success.border
            } else {
                p.k.c.border_base
            },
        )
        .child(
            text(pill_label)
                .font(p.k.ty.small_medium)
                .color(if committed { p.success() } else { p.text() }),
        );

    let mut top = row()
        .key("slide-navigation")
        .w_full()
        .center_y()
        .gap(10.)
        .child(pill)
        .child(
            text(date_text)
                .key("planner-date")
                .font(p.k.ty.small)
                .color(p.muted())
                .grow(1.),
        );

    if !committed {
        let total = all_slides.len();
        let remaining = total.saturating_sub(index);
        let mins = remaining.div_ceil(2).max(1);
        let step_meta = if weekly {
            format!("Step {} of {total} · then today’s plan", index + 1)
        } else {
            format!("Step {} of {total} · ~{mins} min left", index + 1)
        };
        top = top.child(
            text(step_meta)
                .key("step-progress-text")
                .font(p.k.ty.small.features("'tnum' 1"))
                .color(p.muted()),
        );
    }

    // Reserve the exact same 3 px progress strip on DayView so the card never jumps vertically.
    let progress_strip = if committed {
        row()
            .key("progress-segments")
            .w_full()
            .child(el().grow(1.).h(3.).radius(999.).fill(p.k.c.success.fg))
    } else {
        let mut segments = row().key("progress-segments").w_full().gap(6.);
        for i in 0..all_slides.len() {
            segments = segments.child(el().grow(1.).h(3.).radius(999.).fill(if i <= index {
                p.text()
            } else {
                p.k.c.border_base
            }));
        }
        segments
    };

    col().w_full().gap(12.).child(top).child(progress_strip)
}

fn action_bar(planner: &Planner, index: usize, p: Paint) -> El {
    let editing = planner.edit_backup.is_some();
    let primary_label = match planner.slide {
        Slide::Picture => "Start today’s plan →",
        Slide::Focus => "Save weekly focus →",
        Slide::Breakdown if editing => "Save changes ↵",
        Slide::Breakdown => "Commit to today ↵",
        Slide::DayView if planner.reopened => "Done ↵",
        Slide::DayView => "Start the day ↵",
        _ => "Continue →",
    };

    let hint = match planner.slide {
        Slide::Breakdown if editing && !planner.steps.is_empty() => {
            "Enter adds · Enter on empty or ⌘Enter saves"
        }
        Slide::Breakdown if editing => "Enter adds step · ⌘Enter saves",
        Slide::Breakdown if !planner.steps.is_empty() => {
            "Enter adds · Enter on empty or ⌘Enter commits"
        }
        Slide::Breakdown => "Enter adds step · ⌘Enter commits",
        Slide::DayView => "Click steps to check off",
        Slide::Picture => "← → Navigate · Enter continues",
        Slide::Yesterday => match planner
            .ledger
            .previous(planner.date)
            .and_then(|d| d.delivered)
        {
            None => "Choose Yes or Not yet to continue",
            Some(_) => "Enter to continue",
        },
        Slide::Today | Slide::Progress => "Enter continues · ⇧Enter new line",
        Slide::Goal | Slide::Focus | Slide::Outcome => "Enter next field · ⌘Enter continues",
    };

    let mut bar = row()
        .key("slide-actions")
        .w_full()
        .pt(14.)
        .center_y()
        .gap(10.);

    if planner.slide == Slide::DayView {
        bar = bar.child(
            Button::new("Edit plan")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Regular)
                .on_press_tx(|state: &mut Planner, tx| state.edit_plan(tx))
                .build(p.k)
                .key("planner-edit"),
        );
    } else if editing && index == 0 {
        bar = bar.child(
            Button::new("Cancel")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Regular)
                .on_press_tx(|state: &mut Planner, tx| {
                    state.cancel_edit(tx);
                })
                .build(p.k)
                .key("planner-cancel-edit"),
        );
    } else if index > 0 {
        bar = bar.child(
            Button::new("← Back")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Regular)
                .on_press_tx(|state: &mut Planner, tx| state.back(tx))
                .build(p.k)
                .key("planner-back"),
        );
    }

    bar.child(el().grow(1.))
        .child(
            text(hint)
                .key("slide-shortcut-hint")
                .font(p.k.ty.small)
                .color(p.muted())
                .shrink()
                .truncate(),
        )
        .child(
            Button::new(primary_label)
                .variant(ButtonVariant::Submit)
                .control_size(ControlSize::Large)
                .on_press_tx(|state: &mut Planner, tx| state.advance(tx))
                .build(p.k)
                .key("planner-next"),
        )
}

pub(super) fn view(planner: &Planner, cx: &mut Cx<Planner>) -> El {
    let p = Paint::new(cx.system_dark(), cx.env().scale);
    let all_slides = planner.slides();
    let index = all_slides
        .iter()
        .position(|slide| *slide == planner.slide)
        .unwrap_or(0);
    let today_done = planner.today_committed();
    let editing = planner.edit_backup.is_some();

    let heading_title = if planner.slide == Slide::DayView && planner.reopened {
        "Today’s plan"
    } else {
        planner.slide.title()
    };

    let subtitle = if planner.slide == Slide::DayView {
        match planner.option_mode {
            super::OptionPeekMode::TapOrHold => {
                "Check off steps here, tap or hold ⌥ Option, or use Today’s Steps in the menu bar."
            }
            super::OptionPeekMode::TapOnly => {
                "Check off steps here, tap ⌥ Option to pin Today’s Steps, or hold it to glance."
            }
            super::OptionPeekMode::MenuBarOnly => {
                "Check off steps here, or open Today’s Steps from Hex’s menu bar anytime."
            }
        }
    } else {
        planner.slide.subtitle()
    };

    let body = match planner.slide {
        Slide::Progress => slides::weekly_review(planner, p),
        Slide::Goal => slides::goal_slide(planner, p),
        Slide::Focus => slides::focus_slide(planner, p),
        Slide::Picture => slides::picture(planner, p),
        Slide::Yesterday => slides::yesterday(planner, p),
        Slide::Today => slides::today_slide(planner, p),
        Slide::Outcome => slides::outcome_slide(planner, p),
        Slide::Breakdown => slides::breakdown(planner, p),
        Slide::DayView => slides::day_view(planner, p),
    };

    // Keep outer and card padding identical across every slide so the header never jumps.
    let mut card = col()
        .key("planner-card")
        .w_full()
        .padding(26.)
        .gap(if planner.slide == Slide::Progress {
            12.
        } else {
            18.
        })
        .radius(p.k.radius.xl)
        .fill(p.k.c.layer_01)
        .border(p.hair, p.k.c.border_base)
        .child(
            col()
                .w_full()
                .gap(4.)
                .child(
                    text(heading_title)
                        .role("heading")
                        .level(1)
                        .font(crate::rune_ui::controls::PLANNER_HEADING)
                        .color(p.text()),
                )
                .child(text(subtitle).font(p.k.ty.ui).color(p.muted()).wrap()),
        )
        .child(body);

    if let Some(error) = &planner.error {
        card = card.child(
            row()
                .key("planner-error-box")
                .w_full()
                .px(12.)
                .py(10.)
                .radius(p.k.radius.md)
                .fill(p.k.c.danger.bg)
                .border(p.hair, p.k.c.danger.border)
                .child(
                    text(error)
                        .key("planner-error")
                        .role("alert")
                        .font(p.k.ty.ui)
                        .color(p.danger())
                        .wrap(),
                ),
        );
    }

    if let Some(note) = &planner.login_note {
        card = card.child(
            text(note)
                .key("planner-login-note")
                .font(p.k.ty.small)
                .color(p.muted())
                .wrap(),
        );
    }

    card = card
        .child(el().w_full().h(p.hair).fill(p.k.c.border_muted))
        .child(action_bar(planner, index, p));

    let content = col()
        .key("planner-content")
        .focusable(true)
        .w_full()
        .max_w(640.)
        .gap(14.)
        .child(header_bar(planner, &all_slides, index, p))
        .child(p.k.elevate(card, &p.k.elevation.raised));

    let footer = if planner.slide == Slide::DayView {
        // Avoid duplicating "Start the day" / "Done" in the footer; Esc still closes cleanly.
        row()
            .key("planner-footer")
            .w_full()
            .px(24.)
            .py(12.)
            .center_y()
            .child(el().grow(1.))
            .child(
                text("Esc to close")
                    .font(p.k.ty.small)
                    .color(p.muted())
                    .px(8.)
                    .key("planner-bypass"),
            )
    } else {
        let bypass_label = if editing {
            "Cancel edits · Esc"
        } else if today_done && !planner.ledger.week_due(planner.date) {
            "Close · Esc"
        } else {
            "Not now · remind in 5 min"
        };
        row()
            .key("planner-footer")
            .w_full()
            .px(24.)
            .py(12.)
            .center_y()
            .child(el().grow(1.))
            .child(
                Button::new(bypass_label)
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .on_press_tx(|state: &mut Planner, tx| {
                        if !state.cancel_edit(tx) {
                            state.dismiss(crate::history::now_ms() / 1_000);
                        }
                    })
                    .build(p.k)
                    .key("planner-bypass"),
            )
    };

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
                .child(
                    col()
                        .w_full()
                        .items_center()
                        .px(28.)
                        .pt(40.)
                        .pb(24.)
                        .child(content),
                ),
        )
        .child(footer)
}
