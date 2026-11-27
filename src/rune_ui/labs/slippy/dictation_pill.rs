use std::sync::OnceLock;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use rune::{
    Anim, Cx, DisplayTarget, El, KeyPolicy, OverlayHandle, OverlayInput, OverlayOptions,
    OverlaySpec, ScreenEdge, Tx, el, row, text,
};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant};
use rune_theme_opencode::icons;

use super::{SEND, SENDING, SENT, TRANSCRIPT, result};
use crate::desktop::i18n::tr;
use crate::rune_ui::controls::{CAPSULE, Paint};
use crate::slippy::{self, Error, Task};

const OFFER_TIME: Duration = Duration::from_secs(4);
const RESULT_TIME: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(100);

static OVERLAY: OnceLock<OverlayHandle<Pill>> = OnceLock::new();

#[derive(Default)]
struct Pill {
    phase: Phase,
    deadline: Option<Instant>,
    paused: Option<Duration>,
}

#[derive(Default)]
enum Phase {
    #[default]
    Hidden,
    Offer(String),
    Sending(Receiver<Result<Task, Error>>),
    Result(Result<Task, Error>),
}

pub(crate) fn install<S: 'static>(app: &mut rune::App<S>) {
    crate::slippy::notify::install();
    let overlay = app.overlay(
        OverlaySpec {
            name: "Hex Slippy dictation",
            size: (380.0, 52.0),
            edge: ScreenEdge::Top,
            margin: 44.0,
            align: rune::Align::Center,
            inset: 0.0,
            interactive: true,
            view,
        }
        .options(OverlayOptions {
            display: DisplayTarget::UnderPointer,
            keys: KeyPolicy::None,
            input: OverlayInput::Regions,
            ..Default::default()
        })
        .macos_level(3)
        .poll(poll)
        .release_gpu_when_hidden(),
        Pill::default(),
    );
    let _ = OVERLAY.set(overlay);
}

pub(crate) fn offer_dictation(words: &str) {
    if !slippy::active() || words.trim().is_empty() {
        return;
    }
    let Some(overlay) = OVERLAY.get() else {
        return;
    };
    let words = words.to_owned();
    let offered_at = Instant::now();
    overlay.update(move |state, tx| {
        if slippy::active()
            && offered_at.elapsed() < OFFER_TIME
            && !matches!(state.phase, Phase::Sending(_))
        {
            state.phase = Phase::Offer(words);
            state.deadline = Some(offered_at + OFFER_TIME);
            state.paused = None;
            overlay.show();
            tx.wake_after(OFFER_TIME.saturating_sub(offered_at.elapsed()));
        }
    });
}

impl Pill {
    fn hide(&mut self) {
        *self = Self::default();
        if let Some(overlay) = OVERLAY.get() {
            overlay.hide();
        }
    }

    fn hover(&mut self, on: bool, tx: &mut Tx) {
        if !matches!(self.phase, Phase::Offer(_)) {
            return;
        }
        if on {
            if let Some(at) = self.deadline.take() {
                self.paused = Some(at.saturating_duration_since(Instant::now()));
            }
        } else if let Some(remaining) = self.paused.take() {
            self.deadline = Some(Instant::now() + remaining);
            tx.wake_after(remaining);
        }
    }

    fn send(&mut self, tx: &mut Tx) {
        if let Phase::Offer(words) = std::mem::take(&mut self.phase) {
            let response = slippy::handle().and_then(|handle| handle.dictation(words));
            self.deadline = None;
            self.paused = None;
            match response {
                Ok(receiver) => {
                    self.phase = Phase::Sending(receiver);
                    tx.wake_after(TICK);
                }
                Err(error) => self.finished(Err(error), tx),
            }
        }
    }

    fn finished(&mut self, result: Result<Task, Error>, tx: &mut Tx) {
        self.phase = Phase::Result(result);
        self.deadline = Some(Instant::now() + RESULT_TIME);
        tx.wake_after(RESULT_TIME);
    }
}

fn poll(state: &mut Pill, _: f64, tx: &mut Tx) -> bool {
    if matches!(state.phase, Phase::Hidden) {
        return false;
    }
    if !slippy::active() {
        state.hide();
        return true;
    }
    if let Some(deadline) = state.deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            state.hide();
            return true;
        }
        tx.wake_after(remaining.min(TICK));
    }
    if let Phase::Sending(receiver) = &state.phase
        && let Some(result) = result(receiver)
    {
        state.finished(result, tx);
        return true;
    }
    if state.deadline.is_none() {
        // Recheck the account gate even if a pointer has paused the timer.
        tx.wake_after(TICK);
    }
    false
}

fn view(state: &Pill, cx: &mut Cx<Pill>) -> El {
    let p = Paint::new(cx.system_dark(), cx.env().scale);
    let root = el()
        .key("slippy-pill")
        .w_full()
        .h_full()
        .hover_motion_within(Anim::INSTANT, Anim::INSTANT)
        .px(CAPSULE.window_inset)
        .py(CAPSULE.window_inset);
    let action = match &state.phase {
        Phase::Hidden => return root,
        Phase::Offer(_) => Button::new(tr(SEND))
            .variant(ButtonVariant::Contrast)
            .control_size(ControlSize::Small)
            .help(tr(TRANSCRIPT))
            .on_press_tx(Pill::send)
            .build(p.k)
            .key("slippy-pill-send"),
        Phase::Sending(_) => text(tr(SENDING))
            .key("slippy-pill-sending")
            .font(p.strong())
            .color(p.text()),
        Phase::Result(result) => {
            let label = match result {
                Ok(_) => SENT,
                Err(error) => error.message(),
            };
            text(tr(label))
                .key("slippy-pill-result")
                .font(p.strong())
                .color(p.text())
        }
    };
    root.child(
        row()
            .key("slippy-pill-surface")
            .w_full()
            .h_full()
            .center_y()
            .px(8.0)
            .gap(8.0)
            .fill(p.k.c.bg_base)
            .radius(CAPSULE.radius)
            .border(p.hair, p.k.c.border_base)
            .hit()
            .on_hover_change(Pill::hover)
            .child(action)
            .children(matches!(state.phase, Phase::Offer(_)).then(|| {
                text(tr(TRANSCRIPT))
                    .key("slippy-pill-privacy")
                    .font(p.description())
                    .color(p.muted())
                    .shrink()
                    .truncate()
            }))
            .child(el().key("slippy-pill-space").grow(1.0))
            .child(
                IconButton::new(&icons::CLOSE_SMALL)
                    .variant(IconButtonVariant::Ghost)
                    .control_size(ControlSize::Small)
                    .help(tr("Close"))
                    .on_press(Pill::hide)
                    .build(p.k)
                    .key("slippy-pill-dismiss"),
            ),
    )
}
