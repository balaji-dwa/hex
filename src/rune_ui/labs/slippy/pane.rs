use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use rune::{El, TextEdit, Tx, col, el, row, text};
use rune_kit::{Badge, TextArea};

use super::{Progress, member, result, visible};
use crate::desktop::i18n::{event_age, tr};
use crate::rune_ui::{
    Destination, Pane, Ui,
    controls::{self, ButtonKind, Paint},
    list_detail as ld,
};
use crate::slippy::{self, Detail, Error, Role, Status, Task, valid_ts};
use crate::team_auth::AuthError;

pub(super) const RESULT_TICK: Duration = Duration::from_millis(100);
const RESULT_TIME: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(in crate::rune_ui) struct Data {
    pub(super) snapshot: slippy::Snapshot,
    account: Option<String>,
    draft: TextEdit,
    reply: Option<Receiver<Result<Task, Error>>>,
    reply_error: Option<Error>,
    pub(super) history: Option<(u64, Progress)>,
    link: Option<String>,
}

pub(in crate::rune_ui) fn poll(ui: &mut Ui, tx: &mut Tx) -> bool {
    if !ui.live() {
        return false;
    }
    let mut changed = false;
    let account = member(ui).map(str::to_owned);
    if account != ui.lab_store.slippy.account {
        ui.lab_store.slippy.account = account.clone();
        ui.lab_store.slippy.draft.clear();
        ui.lab_store.slippy.reply = None;
        ui.lab_store.slippy.reply_error = None;
        ui.lab_store.slippy.history = None;
        changed = true;
    }
    let pane = ui.pane == Pane::Slippy
        && !ui.labs_open
        && ui.window.as_ref().is_some_and(rune::AppHandle::is_visible);
    if let Some(waker) = ui.modes.waker.clone() {
        slippy::reconcile(account.as_deref(), pane, move || waker());
    }
    let snapshot = slippy::snapshot();
    if snapshot.needs_reauth
        && let Some(account) = &account
    {
        ui.lab_store
            .team_auth
            .needs_reauth(account, AuthError::Refresh);
        slippy::sign_out();
        ui.lab_store.slippy.snapshot = Default::default();
        ui.lab_store.slippy.draft.clear();
        ui.lab_store.slippy.reply = None;
        ui.lab_store.slippy.reply_error = None;
        ui.lab_store.slippy.history = None;
        crate::rune_ui::leave_hidden_pane(ui);
        return true;
    }
    changed |= ui.lab_store.slippy.snapshot.version != snapshot.version;
    ui.lab_store.slippy.snapshot = snapshot;
    changed |= crate::rune_ui::leave_hidden_pane(ui);
    if account.is_some()
        && slippy::active()
        && let Some(ts) = ui.lab_store.slippy.link.take()
    {
        open_link(ui, ts, tx);
        changed = true;
    } else if ui.lab_store.team_auth.signed_out() {
        ui.lab_store.slippy.link = None;
    }
    if let Some(receiver) = &ui.lab_store.slippy.reply {
        match result(receiver) {
            Some(result) => {
                ui.lab_store.slippy.reply = None;
                match result {
                    Ok(_) => ui.lab_store.slippy.draft.clear(),
                    Err(error) => ui.lab_store.slippy.reply_error = Some(error),
                }
                changed = true;
            }
            None => tx.wake_after(RESULT_TICK),
        }
    }
    if let Some((_, progress)) = &mut ui.lab_store.slippy.history {
        match progress {
            Progress::Pending(receiver) => {
                if let Some(result) = result(receiver) {
                    *progress = Progress::Finished(result, Instant::now());
                    changed = true;
                }
                tx.wake_after(if progress.pending() {
                    RESULT_TICK
                } else {
                    RESULT_TIME
                });
            }
            Progress::Finished(_, finished) => {
                if finished.elapsed() >= RESULT_TIME {
                    ui.lab_store.slippy.history = None;
                    changed = true;
                } else {
                    tx.wake_after(RESULT_TIME.saturating_sub(finished.elapsed()));
                }
            }
        }
    }
    changed
}

pub(in crate::rune_ui) fn open_link(ui: &mut Ui, ts: String, tx: &mut Tx) {
    if !crate::labs::team_auth(&ui.settings) || !valid_ts(&ts) {
        return;
    }
    if !visible(ui) || !slippy::active() {
        ui.lab_store.slippy.link = Some(ts);
        return;
    }
    crate::rune_ui::show(ui, Destination::Settings, tx);
    ui.select_pane(Pane::Slippy, tx);
    select(ui, ts);
}

pub(in crate::rune_ui) fn view(ui: &Ui, p: Paint) -> (Vec<El>, El) {
    let refresh = controls::button(
        p,
        "slippy-refresh",
        tr("Refresh"),
        ButtonKind::Secondary,
        |ui, _| {
            if visible(ui)
                && let Ok(handle) = slippy::handle()
            {
                handle.refresh();
            }
        },
    );
    let list = col()
        .key("slippy-tasks")
        .children(ui.lab_store.slippy.snapshot.tasks.iter().map(|task| {
            let selected =
                ui.lab_store.slippy.snapshot.selection.as_deref() == Some(task.ts.as_str());
            let unread = ui.lab_store.slippy.snapshot.unread.contains(&task.ts);
            let ts = task.ts.clone();
            let ink = match task.status {
                Status::Failed => p.danger(),
                Status::Done => p.success(),
                Status::Running | Status::Waiting => p.accent(),
                Status::Queued => p.faint(),
            };
            ld::item_with_press(
                p,
                ld::Item {
                    key: format!("slippy-task-{ts}"),
                    title: task.title.clone(),
                    meta: tr(task.status.label()).into(),
                    meta_ink: Some(ink),
                    note: unread.then(|| tr("Unread").into()),
                    time: event_age(task.updated_at),
                    hover: Vec::new(),
                    selected,
                },
                |item| {
                    let item = item.child(
                        controls::status_dot(if unread { p.accent() } else { ink }).a11y_hidden(),
                    );
                    let ts = ts.clone();
                    crate::rune_ui::lists::select_at_once(item, selected, move |ui| {
                        select(ui, ts.clone())
                    })
                },
            )
        }));
    let list = ld::list_column(p, el().key("slippy-list-top"), "slippy-task-scroll", list);
    let detail = match &ui.lab_store.slippy.snapshot.detail {
        Some(Ok(detail)) => thread(ui, p, detail),
        Some(Err(error)) => ld::empty_state(
            p,
            "slippy-detail-error",
            &crate::rune_ui::icons::PLAIN_BOOK,
            tr(error.message()),
            None,
        ),
        None => {
            let label = if let Some(error) = ui.lab_store.slippy.snapshot.error {
                error.message()
            } else if ui.lab_store.slippy.snapshot.selection.is_some() {
                "Loading…"
            } else if ui.lab_store.slippy.snapshot.tasks.is_empty() {
                "No Slippy tasks yet"
            } else {
                "Select a Slippy task"
            };
            ld::empty_state(
                p,
                "slippy-empty",
                &crate::rune_ui::icons::PLAIN_BOOK,
                tr(label),
                None,
            )
        }
    };
    (vec![refresh], ld::body(p, "slippy", list, detail))
}

fn select(ui: &mut Ui, ts: String) {
    if visible(ui)
        && let Ok(handle) = slippy::handle()
    {
        if ui.lab_store.slippy.snapshot.selection.as_deref() != Some(&ts) {
            ui.lab_store.slippy.draft.clear();
            ui.lab_store.slippy.reply = None;
            ui.lab_store.slippy.reply_error = None;
        }
        handle.select(ts);
    }
}

fn thread(ui: &Ui, p: Paint, detail: &Detail) -> El {
    let messages = col()
        .key("slippy-messages")
        .gap(16.0)
        .children(detail.messages.iter().map(|message| {
            let label = match message.role {
                Role::User => tr("You"),
                Role::Assistant => "Slippy",
            };
            col()
                .key(format!("slippy-message-{}", message.ts))
                .gap(4.0)
                .child(text(label).key("speaker").font(p.strong()).color(p.text()))
                .child(
                    text(&message.text)
                        .key("message-text")
                        .font(p.k.ty.body)
                        .color(p.text())
                        .wrap(),
                )
                .children(
                    message
                        .files
                        .iter()
                        .map(|file| Badge::new(&file.name).build(p.k).key(&file.id)),
                )
        }));
    let ts = detail.task.ts.clone();
    let waiting = ui.lab_store.slippy.reply.is_some();
    let send = controls::button(
        p,
        "slippy-reply-send",
        tr("Send to Slippy"),
        ButtonKind::Primary,
        move |ui, tx| {
            if !visible(ui)
                || ui.lab_store.slippy.reply.is_some()
                || ui.lab_store.slippy.draft.text.trim().is_empty()
            {
                return;
            }
            ui.lab_store.slippy.reply_error = None;
            match slippy::handle()
                .and_then(|handle| handle.reply(ts.clone(), ui.lab_store.slippy.draft.text.clone()))
            {
                Ok(receiver) => {
                    ui.lab_store.slippy.reply = Some(receiver);
                    tx.wake_after(RESULT_TICK);
                }
                Err(error) => ui.lab_store.slippy.reply_error = Some(error),
            }
        },
    )
    .disabled(waiting || ui.lab_store.slippy.draft.text.trim().is_empty());
    col()
        .key("slippy-thread")
        .grow(1.0)
        .shrink()
        .gap(12.0)
        .child(ld::card_header(
            ld::card_title(p, &detail.task.title),
            vec![ld::meta_text(p, tr(detail.task.status.label()))],
            Vec::new(),
        ))
        .child(ld::card_scroll_sticky(
            p,
            "slippy-thread-scroll",
            messages,
            true,
        ))
        .children(ui.lab_store.slippy.reply_error.map(|error| {
            text(tr(error.message()))
                .key("slippy-reply-error")
                .font(p.description())
                .color(p.danger())
        }))
        .child(
            TextArea::new(&ui.lab_store.slippy.draft)
                .placeholder(tr("Follow up with Slippy…"))
                .rows(2)
                .width(None)
                .disabled(waiting)
                .on_edit(|ui: &mut Ui| &mut ui.lab_store.slippy.draft)
                .build(p.k)
                .key("slippy-reply-input"),
        )
        .child(row().key("slippy-reply-controls").pb(16.0).child(send))
}
