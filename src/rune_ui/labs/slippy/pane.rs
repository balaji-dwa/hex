use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use rune::{Align, Color, Cursor, El, SurfaceFit, TextEdit, Tx, Waker, col, el, row, text};
use rune_kit::{Badge, TextArea};

use super::{Progress, member, result, visible};
use crate::desktop::i18n::{event_age, tr};
use crate::rune_ui::{
    Destination, Pane, Ui,
    controls::{self, ButtonKind, METRICS, Paint},
    list_detail as ld, lists,
};
use crate::slippy::{self, Detail, Error, FileKey, Role, Status, Task, valid_ts};
use crate::team_auth::AuthError;

pub(super) const RESULT_TICK: Duration = Duration::from_millis(100);
const RESULT_TIME: Duration = Duration::from_secs(3);
/// An image attachment's square thumbnail under its message.
pub(super) const THUMB: f32 = 64.0;
/// The largest the preview draws an image, in points.
const PREVIEW: (f32, f32) = (760.0, 520.0);
/// The smallest long edge it draws one at, so a tiny image is still seen.
const PREVIEW_MIN: f32 = 240.0;

#[derive(Default)]
pub(in crate::rune_ui) struct Data {
    pub(super) snapshot: slippy::Snapshot,
    account: Option<String>,
    draft: TextEdit,
    reply: Option<Receiver<Result<Task, Error>>>,
    reply_error: Option<Error>,
    pub(super) history: Option<(u64, Progress)>,
    link: Option<String>,
    /// Decodes fetched attachments off the window's thread.
    images: Option<rune::ImageCache>,
    /// Each thumbnail's decode, kept while the worker holds its bytes.
    pub(super) decoded: HashMap<FileKey, rune::Image>,
    /// The attachment the preview shows.
    pub(super) preview: Option<FileKey>,
    /// The preview's full-size decode, only while it is open.
    pub(super) full: Option<(FileKey, rune::Image)>,
}

impl Data {
    fn reset(&mut self) {
        self.draft.clear();
        self.reply = None;
        self.reply_error = None;
        self.history = None;
        self.decoded.clear();
        self.preview = None;
        self.full = None;
    }

    /// Decodes newly fetched thumbnails once, drops those the worker let
    /// go, closes a preview whose thumbnail is gone, and decodes the open
    /// preview's full-size image once the worker has it (dropping it once
    /// closed). Whether anything changed.
    pub(super) fn sync_images(&mut self, waker: Option<Waker>) -> bool {
        let before = self.decoded.len();
        let thumbs = &self.snapshot.thumbs;
        self.decoded
            .retain(|key, _| thumbs.get(key).is_some_and(Option::is_some));
        let mut changed = self.decoded.len() != before;
        for (key, bytes) in thumbs {
            let Some(bytes) = bytes else { continue };
            if self.decoded.contains_key(key) {
                continue;
            }
            if self.images.is_none() {
                self.images = waker.clone().map(rune::ImageCache::new);
            }
            let Some(cache) = &self.images else {
                break;
            };
            self.decoded.insert(key.clone(), cache.load(bytes.clone()));
            changed = true;
        }
        if self
            .preview
            .as_ref()
            .is_some_and(|key| !self.decoded.contains_key(key))
        {
            self.preview = None;
            changed = true;
        }
        let wanted = self.preview.as_ref().and_then(|key| {
            self.snapshot
                .preview_image
                .as_ref()
                .filter(|(loaded, _)| loaded == key)
        });
        match wanted {
            Some((key, bytes)) => {
                if self.full.as_ref().is_none_or(|(shown, _)| shown != key) {
                    if self.images.is_none() {
                        self.images = waker.map(rune::ImageCache::new);
                    }
                    if let Some(cache) = &self.images {
                        self.full = Some((key.clone(), cache.load(bytes.clone())));
                        changed = true;
                    }
                }
            }
            None => changed |= self.full.take().is_some(),
        }
        changed
    }

    fn ready(&self, key: &FileKey) -> Option<&rune::Image> {
        self.decoded
            .get(key)
            .filter(|image| image.is_ready() && image.error().is_none())
    }

    /// The preview's full-size decode of `key`, once ready.
    fn full_ready(&self, key: &FileKey) -> Option<&rune::Image> {
        self.full
            .as_ref()
            .filter(|(shown, image)| shown == key && image.is_ready() && image.error().is_none())
            .map(|(_, image)| image)
    }
}

/// Opens attachment `key`'s preview: its thumbnail scaled up until the
/// full-size image the worker loads is decoded.
fn open_preview(ui: &mut Ui, key: FileKey) {
    ui.lab_store.slippy.preview = Some(key.clone());
    if let Ok(handle) = slippy::handle() {
        handle.open_preview(key);
    }
}

/// Closes the preview and lets its full-size image go.
fn close_preview(ui: &mut Ui) {
    ui.lab_store.slippy.preview = None;
    ui.lab_store.slippy.full = None;
    if let Ok(handle) = slippy::handle() {
        handle.close_preview();
    }
}

pub(in crate::rune_ui) fn poll(ui: &mut Ui, tx: &mut Tx) -> bool {
    if !ui.live() {
        return false;
    }
    let mut changed = false;
    let account = member(ui).map(str::to_owned);
    if account != ui.lab_store.slippy.account {
        ui.lab_store.slippy.account = account.clone();
        ui.lab_store.slippy.reset();
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
        ui.lab_store.slippy.reset();
        crate::rune_ui::leave_hidden_pane(ui);
        return true;
    }
    changed |= ui.lab_store.slippy.snapshot.version != snapshot.version;
    ui.lab_store.slippy.snapshot = snapshot;
    let waker = ui.modes.waker.clone();
    changed |= ui.lab_store.slippy.sync_images(waker);
    if ui.lab_store.slippy.preview.is_none() && ui.lab_store.slippy.snapshot.preview_image.is_some()
    {
        close_preview(ui);
    }
    if ui.lab_store.slippy.preview.is_some() && ui.pane != Pane::Slippy {
        close_preview(ui);
        changed = true;
    }
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

/// A task row's status line and dot: finished tasks drop both "Done" and
/// "Unread" text, leaving only the unread dot when a finished task still
/// needs a look.
pub(super) fn row_status(p: Paint, status: Status, unread: bool) -> (&'static str, Option<Color>) {
    let ink = match status {
        Status::Done => return ("", unread.then(|| p.accent())),
        Status::Failed => p.danger(),
        Status::Running | Status::Waiting => p.accent(),
        Status::Queued => p.faint(),
    };
    (status.label(), Some(ink))
}

fn task_row(p: Paint, task: &Task, selected: bool, unread: bool) -> El {
    let c = &p.k.c;
    let ts = task.ts.clone();
    let (label, ink) = row_status(p, task.status, unread);
    let dot_ink = ink.map(|ink| if unread { p.accent() } else { ink });
    let time = event_age(task.updated_at);
    let described = [
        (!label.is_empty()).then(|| tr(label)),
        unread.then(|| tr("Unread")),
        Some(time.as_str()),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    let words = col()
        .key("words")
        .gap(2.0)
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .child(
            text(task.title.clone())
                .key("title")
                .font(p.k.ty.ui_medium)
                .color(p.text())
                .truncate()
                .a11y_hidden(),
        )
        .children((!label.is_empty()).then(|| {
            text(tr(label))
                .key("meta")
                .font(p.k.ty.small)
                .color(ink.unwrap_or(p.muted()))
                .truncate()
                .a11y_hidden()
        }));
    let end = row()
        .key("end")
        .gap(8.0)
        .center_y()
        .child(
            text(time)
                .key("time")
                .font(p.tabular(p.meta()))
                .color(p.faint())
                .align(Align::End)
                .a11y_hidden(),
        )
        .children(dot_ink.map(controls::status_dot));
    let mut row_el = row()
        .key(format!("slippy-task-{ts}"))
        .role("listitem")
        .label(task.title.clone())
        .description(described)
        .min_h(METRICS.item_h)
        .px(METRICS.row_px)
        .py(8.0)
        .gap(12.0)
        .center_y()
        .radius(p.k.radius.md)
        .hover(c.hover)
        .hover_group()
        .child(words)
        .child(end);
    if selected {
        row_el = row_el.fill(c.layer_02);
    }
    lists::source_item(
        lists::select_at_once(row_el, selected, move |ui| select(ui, ts.clone())),
        selected,
    )
}

pub(in crate::rune_ui) fn view(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> (Vec<El>, El) {
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
            task_row(p, task, selected, unread)
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
    layer.extend(preview(ui, p).into_iter().flatten());
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
        close_preview(ui);
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
                .children(attachments(ui, p, &detail.task.ts, &message.files))
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
            // A finished thread needs no status line over its answer.
            (detail.task.status != Status::Done)
                .then(|| ld::meta_text(p, tr(detail.task.status.label())))
                .into_iter()
                .collect(),
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

/// A message's files: images as square thumbnails in a row (a placeholder
/// while one loads), anything else, or an image that won't load, by name.
fn attachments(ui: &Ui, p: Paint, ts: &str, files: &[slippy::AttachmentMeta]) -> Option<El> {
    if files.is_empty() {
        return None;
    }
    let data = &ui.lab_store.slippy;
    let c = &p.k.c;
    Some(
        row()
            .key("slippy-files")
            .gap(8.0)
            .wrap()
            .children(files.iter().map(|file| {
                let key = (ts.to_owned(), file.id.clone());
                let failed = !file.mime_type.starts_with("image/")
                    || matches!(data.snapshot.thumbs.get(&key), Some(None))
                    || data
                        .decoded
                        .get(&key)
                        .is_some_and(|image| image.error().is_some());
                if failed {
                    return Badge::new(&file.name).build(p.k).key(&file.id);
                }
                let square = el()
                    .key(format!("slippy-image-{}", file.id))
                    .size(THUMB, THUMB)
                    .radius(p.k.radius.md)
                    .fill(c.layer_01)
                    .clip_soft(0.0);
                let Some(image) = data.ready(&key) else {
                    return square.label(&file.name).a11y_hidden();
                };
                square
                    .role("button")
                    .label(&file.name)
                    .cursor(Cursor::Pointer)
                    .pixels_fit(image.pixels(), SurfaceFit::Fill)
                    .on_press(move |ui: &mut Ui| open_preview(ui, key.clone()))
                    .child(
                        el().absolute(0.0, 0.0)
                            .size(THUMB, THUMB)
                            .radius(p.k.radius.md)
                            .border(p.hair, c.border_weak),
                    )
            })),
    )
}

/// The open attachment, fitted inside the window over a scrim: Escape, a
/// press outside, or Close dismisses it.
fn preview(ui: &Ui, p: Paint) -> Option<[El; 2]> {
    let data = &ui.lab_store.slippy;
    let key = data.preview.as_ref()?;
    let thumb = data.ready(key)?;
    let name = data
        .snapshot
        .detail
        .as_ref()
        .and_then(|detail| detail.as_ref().ok())
        .and_then(|detail| {
            detail
                .messages
                .iter()
                .flat_map(|message| &message.files)
                .find(|file| file.id == key.1)
        })
        .map_or_else(|| key.1.clone(), |file| file.name.clone());
    let (image, (width, height)) = match data.full_ready(key) {
        Some(full) => {
            // Hex's uploads are Retina captures: two pixels to the point.
            let [width, height] = full.size()?.map(|edge| edge.max(1) as f32 / 2.0);
            let contain = (PREVIEW.0 / width).min(PREVIEW.1 / height);
            let fit = contain.min((PREVIEW_MIN / width.max(height)).max(1.0));
            (full, (width * fit, height * fit))
        }
        None => {
            // Until it decodes, the thumbnail fills the preview's bounds.
            let [width, height] = thumb.size()?.map(|edge| edge.max(1) as f32);
            let fit = (PREVIEW.0 / width).min(PREVIEW.1 / height);
            (thumb, (width * fit, height * fit))
        }
    };
    let close = close_preview;
    let panel = col()
        .key("slippy-preview")
        .role("dialog")
        .label(&name)
        .min_w(320.0)
        .padding(12.0)
        .gap(12.0)
        .radius(p.k.radius.xl)
        .fill(p.k.c.bg_base)
        .border(p.hair, p.k.c.border_weak)
        // Presses inside the panel stay inside.
        .on_press(|_: &mut Ui| {})
        .child(
            row()
                .key("slippy-preview-header")
                .gap(12.0)
                .center_y()
                .child(
                    text(&name)
                        .key("slippy-preview-name")
                        .font(p.strong())
                        .color(p.text())
                        .grow(1.0)
                        .shrink()
                        .truncate(),
                )
                .child(controls::button(
                    p,
                    "slippy-preview-close",
                    tr("Close"),
                    ButtonKind::Secondary,
                    move |ui, _| close(ui),
                )),
        )
        .child(
            el().key("slippy-preview-image")
                .self_center()
                .size(width, height)
                .radius(p.k.radius.md)
                .clip_soft(0.0)
                .pixels_fit(image.pixels(), SurfaceFit::Fit)
                .label(&name)
                .a11y_hidden(),
        );
    Some(controls::modal_sheet(p, "slippy-preview", panel, close))
}
