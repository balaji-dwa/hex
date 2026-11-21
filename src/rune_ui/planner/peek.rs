//! A read-only view of today's steps, shown while Option is held.

use rune::{Cx, El, ScrollSpec, col, row, text};

use super::{Paint, Planner};

pub(super) const SIZE: (f32, f32) = (360., 520.);

#[derive(Default, PartialEq)]
pub(super) struct Peek {
    shown: bool,
    steps: Vec<String>,
}

impl Peek {
    pub(super) fn snapshot(planner: &Planner, option: bool, away: bool) -> Self {
        let shown = option && !away && !planner.visible && !planner.unreadable;
        let steps = if shown {
            planner
                .ledger
                .days
                .iter()
                .find(|day| day.date == planner.date)
                .into_iter()
                .flat_map(|day| &day.steps)
                .filter(|step| !step.done && !step.text.trim().is_empty())
                .map(|step| step.text.clone())
                .collect()
        } else {
            Vec::new()
        };
        Self { shown, steps }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn present(handle: &rune::OverlayHandle<Self>, next: Self) {
        let own = handle.clone();
        handle.update_if(move |state, tx| {
            if *state == next {
                return false;
            }
            if state.shown != next.shown {
                if next.shown {
                    own.show();
                } else {
                    own.hide();
                }
            }
            *state = next;
            tx.instant();
            true
        });
    }
}

pub(super) fn view(state: &Peek, cx: &mut Cx<Peek>) -> El {
    let p = Paint::new(cx.system_dark(), cx.env().scale);
    let mut cards = col().key("todo-cards").w_full().gap(10.);
    for (index, step) in state.steps.iter().enumerate() {
        let card = row()
            .key(format!("todo-{index}"))
            .w_full()
            .gap(12.)
            .padding(16.)
            .radius(p.k.radius.lg)
            .fill(p.k.c.layer_01)
            .border(p.hair, p.k.c.border_weak)
            .child(
                text(format!("{:02}", index + 1))
                    .font(p.k.ty.small_medium)
                    .color(p.muted()),
            )
            .child(
                text(step)
                    .key("task")
                    .font(p.k.ty.ui)
                    .color(p.text())
                    .wrap_anywhere()
                    .grow(1.)
                    .shrink(),
            );
        cards = cards.child(p.k.elevate(card, &p.k.elevation.floating));
    }
    if state.steps.is_empty() {
        cards = cards.child(
            col()
                .key("no-todos")
                .w_full()
                .padding(16.)
                .radius(p.k.radius.lg)
                .fill(p.k.c.layer_01)
                .border(p.hair, p.k.c.border_weak)
                .child(
                    text("No unfinished steps.")
                        .font(p.k.ty.ui)
                        .color(p.muted())
                        .wrap(),
                ),
        );
    }
    col()
        .key("planner-todos")
        .role("region")
        .label("Today's tasks")
        .w_full()
        .h_full()
        .padding(12.)
        .gap(10.)
        .child(text("Today").font(p.k.ty.small_medium).color(p.muted()))
        .child(
            col()
                .key("todo-scroll")
                .w_full()
                .max_h(SIZE.1 - 54.)
                .shrink()
                .hit()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(cards),
        )
}

#[cfg(test)]
mod tests {
    use super::super::{Ledger, Step, model::Day};
    use super::*;
    use rune::{Env, Runtime, text::TextSystem};

    fn runtime(steps: Vec<String>, dark: bool) -> Runtime<Peek> {
        let text = TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
        let mut rt = super::super::super::offscreen::runtime(
            Peek { shown: true, steps },
            view,
            SIZE,
            Env {
                scale: 1.,
                system_dark: dark,
            },
            text,
        );
        rt.frame(0.);
        rt
    }

    #[test]
    fn option_peek_reads_only_todays_unfinished_steps_and_never_changes_the_plan() {
        let mut ledger = Ledger::default();
        for date in [99, 100] {
            ledger.days.push(Day {
                date,
                steps: vec![
                    Step {
                        text: format!("Unfinished {date}"),
                        done: false,
                    },
                    Step {
                        text: "Already finished".into(),
                        done: true,
                    },
                ],
                ..Default::default()
            });
        }
        let mut planner = Planner::new(ledger, 100, None);
        let before = serde_json::to_vec(&planner.ledger).unwrap();
        let peek = Peek::snapshot(&planner, true, false);
        assert!(peek.shown);
        assert_eq!(peek.steps, ["Unfinished 100"]);
        assert_eq!(serde_json::to_vec(&planner.ledger).unwrap(), before);
        assert!(
            !Peek::snapshot(&planner, false, false).shown,
            "release hides"
        );
        assert!(
            !Peek::snapshot(&planner, true, true).shown,
            "lock/sleep hides"
        );
        planner.visible = true;
        assert!(
            !Peek::snapshot(&planner, true, false).shown,
            "full planner takes priority"
        );
        planner.visible = false;
        planner.unreadable = true;
        assert!(!Peek::snapshot(&planner, true, false).shown);
        planner.unreadable = false;
        planner.date = 101;
        assert!(
            Peek::snapshot(&planner, true, false).steps.is_empty(),
            "never show yesterday as today"
        );
    }

    #[test]
    fn option_peek_wraps_and_scrolls_every_task_without_editors_or_checkboxes() {
        let steps: Vec<_> = (0..100)
            .map(|i| format!("Task {i}: {}", "a long step that wraps ".repeat(5)))
            .collect();
        let mut rt = runtime(steps, true);
        let elements = rt.elements();
        assert_eq!(
            elements
                .iter()
                .filter(|e| e.key.as_deref() == Some("task"))
                .count(),
            100
        );
        assert!(
            !elements
                .iter()
                .any(|e| ["checkbox", "textbox", "button"].contains(&e.role))
        );
        let task = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("task"))
            .unwrap();
        assert!(task.rect[3] > 40. && task.rect[0] + task.rect[2] <= SIZE.0 - 12.);
        let scroll = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("todo-scroll"))
            .unwrap()
            .rect;
        assert!(scroll[3] <= SIZE.1 - 54.);
        assert!(
            !rt.hit_regions().is_empty(),
            "scroll receives the pointer without keyboard focus"
        );
        rt.wheel(scroll[0] + 30., scroll[1] + 30., -100_000.);
        rt.settle_all();
        rt.frame(1.);
        let last = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("todo-99"))
            .unwrap();
        assert!(
            last.rect[1] + last.rect[3] <= SIZE.1,
            "last task is reachable by scrolling"
        );
        let empty = runtime(Vec::new(), false);
        assert!(empty.find("no-todos").is_some());
    }

    #[test]
    fn option_peek_renders() {
        #[cfg(target_os = "linux")]
        let _goldens = super::super::super::tests::Goldens::collect();
        let folder = std::env::var_os("HEX_PLANNER_RENDERS").map(std::path::PathBuf::from);
        if !cfg!(target_os = "linux") && folder.is_none() {
            return;
        }
        #[cfg(target_os = "linux")]
        let gpu = super::super::super::tests::gpu().expect("Planner cards need a GPU");
        #[cfg(not(target_os = "linux"))]
        let gpu = rune::render::Gpu::try_new(rune::render::Gpu::instance(), None).unwrap();
        for dark in [false, true] {
            let mut rt = runtime(
                vec![
                    "Finish the smallest useful Planner improvement".into(),
                    "Review the Rune renders in light and dark mode".into(),
                    "Install the verified build and keep the existing shortcuts working".into(),
                ],
                dark,
            );
            let pixels =
                super::super::super::offscreen::draw(&gpu, &mut rt, SIZE.0 as u32, SIZE.1 as u32);
            if let Some(folder) = &folder {
                std::fs::create_dir_all(folder).unwrap();
                rune::render::write_png(
                    &folder.join(format!(
                        "planner-todos-{}.png",
                        if dark { "dark" } else { "light" }
                    )),
                    SIZE.0 as u32,
                    SIZE.1 as u32,
                    &pixels,
                )
                .unwrap();
            }
            #[cfg(target_os = "linux")]
            super::super::super::tests::check_render(
                "planner-todos",
                dark,
                Some((SIZE.0 as u32, SIZE.1 as u32, pixels)),
            );
        }
    }
}
