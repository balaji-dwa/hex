use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::cache::DiskCache;
use super::client::{Attachment, Client, Detail, Error, Listed, Origin, Role, Task};
use crate::team_auth::{AuthError, Client as AuthClient};

const LIMIT: u8 = 100;
const FAST: Duration = Duration::from_secs(5);
const IDLE: Duration = Duration::from_secs(60);
/// Thumbnails kept in memory at once; the oldest goes first.
pub(super) const MAX_IMAGES: usize = 64;
/// Thread details kept in memory, so selecting a thread again shows it at
/// once and asks again only once its `seq` moves.
pub(super) const MAX_DETAILS: usize = 64;
/// Newest tasks pre-warmed into the disk cache while the Slippy pane is open.
const PREWARM_LIMIT: usize = 12;
/// Polls a finished task waits for its detail to catch up with the list
/// before it notifies with what the detail has.
const LAG_RETRIES: u8 = 3;
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SERVICE: Mutex<Service> = Mutex::new(Service {
    worker: None,
    stopping: None,
});

pub(super) type Wake = Arc<dyn Fn() + Send + Sync>;
/// A task's `ts` and one of its attachment ids.
pub(crate) type FileKey = (String, String);
/// Attachments' thumbnails: the bytes, or `None` when the fetch failed for
/// good (not found, not an image, too large). Absent while it loads.
pub(crate) type Images = HashMap<FileKey, Option<Arc<[u8]>>>;
type Payload = Box<dyn FnOnce() -> Result<(Origin, String, Vec<Attachment>), Error> + Send>;
type Answer = Sender<Result<Task, Error>>;

#[derive(Default)]
struct Service {
    worker: Option<Worker>,
    stopping: Option<Receiver<()>>,
}

pub(super) struct Worker {
    member: String,
    pub(super) handle: Handle,
    thread: JoinHandle<()>,
}

#[derive(Clone)]
pub(crate) struct Handle {
    shared: Arc<Shared>,
    commands: Sender<Command>,
}

struct Shared {
    cancelled: AtomicBool,
    pane: AtomicBool,
    state: Mutex<State>,
    wake: Wake,
    cache: DiskCache,
}

#[derive(Default)]
struct State {
    version: u64,
    tasks: HashMap<String, Task>,
    unread: HashSet<String>,
    selection: Option<String>,
    detail: Option<Result<Detail, Error>>,
    error: Option<Error>,
    needs_reauth: bool,
    /// The newest assistant message `ts` each notification checked, so a
    /// title-only change never notifies again and a new answer does.
    replies: HashMap<String, String>,
    thumbs: ImageCache,
    /// Each thread's newest detail, by `ts`.
    details: HashMap<String, Detail>,
    /// The attachment the preview shows, and its full bytes once loaded.
    preview: Option<(FileKey, Option<Arc<[u8]>>)>,
}

#[derive(Default)]
pub(super) struct ImageCache {
    pub(super) entries: Images,
    order: VecDeque<FileKey>,
}

impl ImageCache {
    pub(super) fn insert(&mut self, key: FileKey, bytes: Option<Arc<[u8]>>) {
        if self.entries.insert(key.clone(), bytes).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > MAX_IMAGES {
            if let Some(old) = self.order.pop_front() {
                self.entries.remove(&old);
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct Snapshot {
    pub version: u64,
    pub tasks: Vec<Task>,
    pub unread: HashSet<String>,
    pub selection: Option<String>,
    pub detail: Option<Result<Detail, Error>>,
    pub error: Option<Error>,
    pub needs_reauth: bool,
    pub thumbs: Images,
    /// The open preview's full-size image, once loaded.
    pub preview_image: Option<(FileKey, Arc<[u8]>)>,
}

enum Command {
    Stop,
    /// A timed poll: the selected thread is asked again only once it moved.
    Poll,
    /// The Refresh button: the selected thread is asked again regardless.
    Refresh,
    Select,
    Preview,
    Submit(Payload, Answer),
    Reply(String, String, Answer),
}

pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

pub(crate) fn handle() -> Result<Handle, Error> {
    SERVICE
        .lock()
        .unwrap()
        .worker
        .as_ref()
        .map(|worker| worker.handle.clone())
        .ok_or(Error::Unavailable)
}

pub(crate) fn snapshot() -> Snapshot {
    handle().map(|handle| handle.snapshot()).unwrap_or_default()
}

pub(crate) fn sign_out() {
    SERVICE.lock().unwrap().stop();
}

/// Starts the worker for `member`, or stops it when there is none (signed
/// out, the lab off, or another account).
pub(crate) fn reconcile(member: Option<&str>, pane: bool, wake: impl Fn() + Send + Sync + 'static) {
    reconcile_with(member, pane, wake, AuthClient::configured);
}

pub(super) fn reconcile_with(
    member: Option<&str>,
    pane: bool,
    wake: impl Fn() + Send + Sync + 'static,
    auth: impl FnOnce() -> Result<AuthClient, AuthError>,
) {
    let mut service = SERVICE.lock().unwrap();
    if service
        .worker
        .as_ref()
        .is_some_and(|worker| Some(worker.member.as_str()) != member)
    {
        service.stop();
    }
    if service
        .stopping
        .as_ref()
        .is_some_and(|done| matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)))
    {
        return;
    }
    service.stopping = None;
    let Some(member) = member else {
        return;
    };
    if service.worker.is_none()
        && let Ok(auth) = auth()
    {
        service.worker = Worker::start(auth, member, pane, Arc::new(wake)).ok();
        ACTIVE.store(service.worker.is_some(), Ordering::Release);
    }
    if let Some(worker) = &service.worker {
        let was_open = worker.handle.shared.pane.swap(pane, Ordering::AcqRel);
        if pane && !was_open {
            let _ = worker.handle.send(Command::Poll);
        }
    }
}

impl Service {
    fn stop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let shared = worker.handle.shared;
            shared.cancelled.store(true, Ordering::Release);
            *shared.state.lock().unwrap() = State::default();
            let _ = worker.handle.commands.send(Command::Stop);
            // ureq can be in an upload. Revoke access now and join outside the UI.
            let wake = shared.wake.clone();
            let (joined, done) = mpsc::channel();
            self.stopping = Some(done);
            thread::spawn(move || {
                let _ = worker.thread.join();
                let _ = joined.send(());
                wake();
            });
        }
    }
}

impl Handle {
    pub(super) fn snapshot(&self) -> Snapshot {
        let state = self.shared.state.lock().unwrap();
        let mut tasks: Vec<_> = state.tasks.values().cloned().collect();
        tasks.sort_by(|a, b| (&b.updated_at, &b.ts).cmp(&(&a.updated_at, &a.ts)));
        Snapshot {
            version: state.version,
            tasks,
            unread: state.unread.clone(),
            selection: state.selection.clone(),
            detail: state.detail.clone(),
            error: state.error,
            needs_reauth: state.needs_reauth,
            thumbs: state.thumbs.entries.clone(),
            preview_image: state
                .preview
                .as_ref()
                .and_then(|(key, bytes)| Some((key.clone(), bytes.clone()?))),
        }
    }

    pub(crate) fn dictation(&self, text: String) -> Result<Receiver<Result<Task, Error>>, Error> {
        self.submit(move || Ok((Origin::Dictation, text, Vec::new())))
    }

    pub(crate) fn image(
        &self,
        origin: Origin,
        bytes: Vec<u8>,
        words: String,
    ) -> Result<Receiver<Result<Task, Error>>, Error> {
        self.submit(move || {
            let (kind, fallback) = if origin == Origin::Screenshot {
                (super::image::Kind::Screenshot, "Screenshot")
            } else {
                (super::image::Kind::Annotation, "Annotation")
            };
            let text = if words.trim().is_empty() {
                fallback.into()
            } else {
                words
            };
            Ok((origin, text, vec![super::image::attachment(kind, &bytes)?]))
        })
    }

    pub(super) fn submit(
        &self,
        prepare: impl FnOnce() -> Result<(Origin, String, Vec<Attachment>), Error> + Send + 'static,
    ) -> Result<Receiver<Result<Task, Error>>, Error> {
        let (sender, receiver) = mpsc::channel();
        self.send(Command::Submit(Box::new(prepare), sender))?;
        Ok(receiver)
    }

    pub(crate) fn reply(
        &self,
        ts: String,
        text: String,
    ) -> Result<Receiver<Result<Task, Error>>, Error> {
        let (sender, receiver) = mpsc::channel();
        self.send(Command::Reply(ts, text, sender))?;
        Ok(receiver)
    }

    /// Selects thread `ts`, showing its cached detail and thumbnails at once
    /// (from memory or disk) without waiting behind a network poll.
    pub(crate) fn select(&self, ts: String) {
        let started = Instant::now();
        let cached = {
            let state = self.shared.state.lock().unwrap();
            state.details.get(&ts).cloned()
        }
        .or_else(|| self.shared.cache.detail(&ts));
        let local_thumbs = cached
            .as_ref()
            .map(|detail| self.shared.cache.local_thumbs(detail))
            .unwrap_or_default();
        let detail_cached = cached.is_some();
        let thumbs_cached = local_thumbs.len();
        self.shared.update(|state| {
            state.unread.remove(&ts);
            if let Some(detail) = &cached {
                remember(state, detail.clone());
            }
            for (key, thumb) in local_thumbs {
                state.thumbs.insert(key, Some(thumb));
            }
            let same = state.selection.as_deref() == Some(&ts);
            if !same {
                state.detail = state.details.get(&ts).cloned().map(Ok);
                state.selection = Some(ts.clone());
                state.preview = None;
            } else if state.detail.is_none()
                && let Some(detail) = state.details.get(&ts).cloned()
            {
                state.detail = Some(Ok(detail));
            }
        });
        tracing::info!(
            target: "hex::slippy",
            ts = %ts,
            detail_cached,
            thumbs_cached,
            ms = started.elapsed().as_millis() as u64,
            "Slippy select"
        );
        let _ = self.send(Command::Select);
    }

    /// Opens attachment `key` full-size: from this Mac's cache at once, or
    /// downloaded by the worker.
    pub(crate) fn open_preview(&self, key: FileKey) {
        let started = Instant::now();
        let bytes = self.shared.cache.full(&key.0, &key.1);
        let size = bytes.as_ref().map_or(0, |data| data.len());
        let missing = bytes.is_none();
        tracing::info!(
            target: "hex::slippy",
            ts = %key.0,
            file = %key.1,
            source = if missing { "download" } else { "disk" },
            bytes = size,
            ms = started.elapsed().as_millis() as u64,
            "Slippy preview open"
        );
        self.shared
            .update(|state| state.preview = Some((key, bytes)));
        if missing {
            let _ = self.send(Command::Preview);
        }
    }

    /// Lets the preview's full-size bytes go.
    pub(crate) fn close_preview(&self) {
        if self.shared.state.lock().unwrap().preview.is_some() {
            self.shared.update(|state| state.preview = None);
        }
    }

    pub(crate) fn refresh(&self) {
        let _ = self.send(Command::Refresh);
    }

    fn send(&self, command: Command) -> Result<(), Error> {
        if self.shared.cancelled.load(Ordering::Acquire) {
            return Err(Error::Unavailable);
        }
        self.commands.send(command).map_err(|_| Error::Unavailable)
    }
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut State)) {
        let mut state = self.state.lock().unwrap();
        if !self.cancelled.load(Ordering::Acquire) {
            f(&mut state);
            state.version = state.version.wrapping_add(1);
            drop(state);
            (self.wake)();
        }
    }

    fn interval(&self) -> Duration {
        if self.pane.load(Ordering::Acquire)
            || self
                .state
                .lock()
                .unwrap()
                .tasks
                .values()
                .any(|task| task.status.active())
        {
            FAST
        } else {
            IDLE
        }
    }
}

impl Worker {
    pub(super) fn start(
        auth: AuthClient,
        member: &str,
        pane: bool,
        wake: Wake,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            cancelled: AtomicBool::new(false),
            pane: AtomicBool::new(pane),
            state: Mutex::default(),
            wake,
            cache: DiskCache::new(auth.cache_dir(member)),
        });
        let (commands, receiver) = mpsc::channel();
        let worker_member = member.to_owned();
        let worker_shared = shared.clone();
        let thread = thread::Builder::new()
            .name("hex-slippy".into())
            .spawn(move || {
                let client = Client {
                    auth: &auth,
                    member: &worker_member,
                    cancel: &worker_shared.cancelled,
                };
                let cursor_path = auth.cursor_path(&worker_member);
                run(&worker_shared, &client, &cursor_path, receiver);
                // Signed out, the lab off, or another account: nothing of
                // this member's stays behind. Quitting Hex keeps both.
                let _ = std::fs::remove_file(cursor_path);
                worker_shared.cache.clear();
            })?;
        Ok(Self {
            member: member.into(),
            handle: Handle { shared, commands },
            thread,
        })
    }
}

fn run(shared: &Shared, client: &Client, path: &Path, commands: Receiver<Command>) {
    // Ask now, not at the first send: a two-second answer would otherwise
    // post before the permission prompt was answered.
    super::notify::authorize();
    let mut cursor = Cursor::load(path);
    let mut prewarmed: HashSet<(String, u64)> = HashSet::new();
    let mut pending: VecDeque<Command> = VecDeque::new();
    poll(shared, client, path, &mut cursor);
    while !shared.cancelled.load(Ordering::Acquire) {
        while let Ok(command) = commands.try_recv() {
            enqueue(&mut pending, command);
        }
        let command = match pending.pop_front() {
            Some(command) => command,
            None => {
                if let Some((ts, seq)) = next_prewarm(shared, &prewarmed) {
                    prewarmed.insert((ts.clone(), seq));
                    prewarm_one(shared, client, &ts, &commands, &mut pending);
                    continue;
                }
                match commands.recv_timeout(shared.interval()) {
                    Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                    Ok(command) => {
                        enqueue(&mut pending, command);
                        while let Ok(more) = commands.try_recv() {
                            enqueue(&mut pending, more);
                        }
                        match pending.pop_front() {
                            Some(next) => next,
                            None => continue,
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => Command::Poll,
                }
            }
        };
        if shared.cancelled.load(Ordering::Acquire) || shared.state.lock().unwrap().needs_reauth {
            return;
        }
        match command {
            Command::Stop => return,
            Command::Poll | Command::Refresh => {
                poll(shared, client, path, &mut cursor);
                detail(shared, client, matches!(command, Command::Refresh));
            }
            Command::Select => detail(shared, client, false),
            Command::Preview => preview(shared, client),
            Command::Submit(prepare, answer) => {
                let mut sent = Vec::new();
                let result = prepare().and_then(|(origin, text, files)| {
                    if shared.cancelled.load(Ordering::Acquire) {
                        return Err(Error::Unavailable);
                    }
                    let created = client.create(origin, &text, &files)?;
                    sent = files
                        .into_iter()
                        .filter(|file| file.mime_type.starts_with("image/"))
                        .map(|file| file.raw)
                        .collect();
                    Ok(created.task)
                });
                let ts = result.as_ref().ok().map(|task| task.ts.clone());
                finish(shared, result, answer);
                // What this Mac sent never downloads again.
                if let Some(ts) = ts {
                    for (index, bytes) in sent.iter().enumerate() {
                        shared.cache.stage(&ts, index, bytes);
                    }
                }
                poll(shared, client, path, &mut cursor);
            }
            Command::Reply(ts, text, answer) => {
                let result = client.reply(&ts, &text).map(|reply| reply.task);
                finish(shared, result, answer);
                poll(shared, client, path, &mut cursor);
                detail(shared, client, false);
            }
        }
    }
}

/// Keeps user actions (`Select`, `Preview`, `Submit`, `Reply`) ahead of
/// background `Poll`s and collapses redundant queued `Poll`/`Select` commands.
fn enqueue(pending: &mut VecDeque<Command>, command: Command) {
    match command {
        Command::Stop => pending.push_front(Command::Stop),
        Command::Poll => {
            if !pending
                .iter()
                .any(|queued| matches!(queued, Command::Poll | Command::Refresh))
            {
                pending.push_back(Command::Poll);
            }
        }
        Command::Select => {
            if pending
                .iter()
                .any(|queued| matches!(queued, Command::Select))
            {
                return;
            }
            let pos = pending
                .iter()
                .position(|queued| matches!(queued, Command::Poll))
                .unwrap_or(pending.len());
            pending.insert(pos, Command::Select);
        }
        Command::Preview => {
            let pos = pending
                .iter()
                .position(|queued| matches!(queued, Command::Poll))
                .unwrap_or(pending.len());
            pending.insert(pos, Command::Preview);
        }
        other => pending.push_back(other),
    }
}

/// Picks the newest recent task whose detail or thumbnails are not yet cached
/// on disk, only while the Slippy pane is open.
fn next_prewarm(shared: &Shared, tried: &HashSet<(String, u64)>) -> Option<(String, u64)> {
    if !shared.pane.load(Ordering::Acquire) {
        return None;
    }
    let candidates: Vec<(String, u64)> = {
        let state = shared.state.lock().unwrap();
        let mut tasks: Vec<_> = state.tasks.values().collect();
        tasks.sort_by(|a, b| (&b.updated_at, &b.ts).cmp(&(&a.updated_at, &a.ts)));
        tasks
            .into_iter()
            .take(PREWARM_LIMIT)
            .filter(|task| !tried.contains(&(task.ts.clone(), task.seq)))
            .map(|task| (task.ts.clone(), task.seq))
            .collect()
    };
    for (ts, seq) in candidates {
        let cached = {
            let state = shared.state.lock().unwrap();
            state.details.get(&ts).cloned()
        }
        .or_else(|| shared.cache.detail(&ts));
        let fresh = cached
            .as_ref()
            .is_some_and(|detail| detail.task.seq >= seq && shared.cache.has_all_thumbs(detail));
        if !fresh {
            return Some((ts, seq));
        }
    }
    None
}

fn prewarm_one(
    shared: &Shared,
    client: &Client,
    ts: &str,
    commands: &Receiver<Command>,
    pending: &mut VecDeque<Command>,
) {
    let started = Instant::now();
    let cached = {
        let state = shared.state.lock().unwrap();
        current(&state, ts)
    }
    .or_else(|| {
        let detail = shared.cache.detail(ts)?;
        let fresh = shared
            .state
            .lock()
            .unwrap()
            .tasks
            .get(ts)
            .is_none_or(|task| detail.task.seq >= task.seq);
        fresh.then_some(detail)
    });
    let detail = match cached {
        Some(detail) => detail,
        None => match client.detail(ts) {
            Ok(detail) => {
                shared.cache.store_detail(&detail);
                shared.update(|state| remember(state, detail.clone()));
                detail
            }
            Err(Error::Auth(AuthError::Refresh)) => {
                report(shared, Error::Auth(AuthError::Refresh));
                return;
            }
            Err(_) => return,
        },
    };
    // Yield right away if the user clicked another task or opened a preview.
    while let Ok(command) = commands.try_recv() {
        enqueue(pending, command);
    }
    if pending
        .iter()
        .any(|command| !matches!(command, Command::Poll))
    {
        return;
    }
    thumbs(shared, client, &detail);
    tracing::info!(
        target: "hex::slippy",
        ts = %ts,
        ms = started.elapsed().as_millis() as u64,
        "Slippy prewarmed task"
    );
}

fn finish(shared: &Shared, result: Result<Task, Error>, answer: Answer) {
    match &result {
        Ok(task) => shared.update(|state| upsert(state, task.clone())),
        Err(error) => report(shared, *error),
    }
    let _ = answer.send(result);
    (shared.wake)();
}

fn report(shared: &Shared, error: Error) {
    shared.update(|state| {
        state.error = Some(error);
        state.needs_reauth |= matches!(error, Error::Auth(auth) if auth.requires_reauth());
    });
}

/// Shows the selected thread: its kept detail while that is as new as the
/// list's task (unless `force`), else a fresh one. A transient failure
/// keeps the kept detail on screen.
fn detail(shared: &Shared, client: &Client, force: bool) {
    if !shared.pane.load(Ordering::Acquire) {
        return;
    }
    let (ts, kept) = {
        let state = shared.state.lock().unwrap();
        let Some(ts) = state.selection.clone() else {
            return;
        };
        let kept = (!force).then(|| current(&state, &ts)).flatten();
        (ts, kept)
    };
    let loaded = match kept {
        Some(detail) => {
            let shown = matches!(
                &shared.state.lock().unwrap().detail,
                Some(Ok(shown)) if *shown == detail
            );
            if !shown {
                shared.update(|state| {
                    if state.selection.as_deref() == Some(&ts) {
                        state.detail = Some(Ok(detail.clone()));
                    }
                });
            }
            Some(detail)
        }
        None => {
            let started = Instant::now();
            let result = client.detail(&ts);
            tracing::info!(
                target: "hex::slippy",
                ts = %ts,
                ok = result.is_ok(),
                ms = started.elapsed().as_millis() as u64,
                "Slippy detail fetched"
            );
            if result == Err(Error::Auth(AuthError::Refresh)) {
                report(shared, Error::Auth(AuthError::Refresh));
            }
            let loaded = result.as_ref().ok().cloned();
            if let Some(detail) = &loaded {
                shared.cache.store_detail(detail);
            }
            shared.update(|state| {
                if let Ok(detail) = &result {
                    remember(state, detail.clone());
                }
                if state.selection.as_deref() != Some(&ts) {
                    return;
                }
                let keep = matches!(result, Err(error) if error != Error::Rejected(404))
                    && matches!(&state.detail, Some(Ok(shown)) if shown.task.ts == ts);
                if !keep {
                    state.detail = Some(result);
                }
            });
            loaded
        }
    };
    if let Some(detail) = loaded {
        thumbs(shared, client, &detail);
    }
}

/// The kept detail of `ts`, if no older than the list's task.
fn current(state: &State, ts: &str) -> Option<Detail> {
    let detail = state.details.get(ts)?;
    state
        .tasks
        .get(ts)
        .is_none_or(|task| detail.task.seq >= task.seq)
        .then(|| detail.clone())
}

/// Keeps `detail` unless an equally new one is kept, letting the
/// least recently updated thread go past [`MAX_DETAILS`].
fn remember(state: &mut State, detail: Detail) {
    if state
        .details
        .get(&detail.task.ts)
        .is_some_and(|kept| kept.task.seq > detail.task.seq)
    {
        return;
    }
    state.details.insert(detail.task.ts.clone(), detail);
    while state.details.len() > MAX_DETAILS {
        let oldest = state
            .details
            .values()
            .filter(|kept| state.selection.as_deref() != Some(&kept.task.ts))
            .min_by_key(|kept| (kept.task.updated_at, kept.task.seq))
            .map(|kept| kept.task.ts.clone());
        match oldest {
            Some(ts) => state.details.remove(&ts),
            None => break,
        };
    }
}

/// Loads the selected thread's image thumbnails not yet in memory: from
/// this Mac's cache, from an upload Hex just sent, or downloaded (and kept)
/// once. A transient failure leaves the image loading for the next detail;
/// one that won't change (not found, not an image) shows the file's name
/// instead.
fn thumbs(shared: &Shared, client: &Client, detail: &Detail) {
    let is_image = |file: &&super::AttachmentMeta| file.mime_type.starts_with("image/");
    // The task's first message carries the files Hex uploaded, in order.
    let sent: Vec<&str> = detail
        .messages
        .iter()
        .find(|message| message.role == Role::User)
        .into_iter()
        .flat_map(|message| message.files.iter().filter(is_image))
        .map(|file| file.id.as_str())
        .collect();
    let wanted = detail
        .messages
        .iter()
        .flat_map(|message| &message.files)
        .filter(is_image)
        .map(|file| (detail.task.ts.clone(), file.id.clone()))
        .take(MAX_IMAGES);
    for key in wanted {
        if shared.cancelled.load(Ordering::Acquire) {
            return;
        }
        if shared
            .state
            .lock()
            .unwrap()
            .thumbs
            .entries
            .contains_key(&key)
        {
            continue;
        }
        if let Some(index) = sent.iter().position(|id| *id == key.1) {
            shared.cache.claim(&key.0, index, &key.1);
        }
        let started = Instant::now();
        if let Some(thumb) = shared.cache.thumb(&key.0, &key.1) {
            tracing::info!(
                target: "hex::slippy",
                ts = %key.0,
                file = %key.1,
                source = "disk",
                bytes = thumb.len(),
                ms = started.elapsed().as_millis() as u64,
                "Slippy thumbnail ready"
            );
            shared.update(|state| state.thumbs.insert(key, Some(thumb)));
            continue;
        }
        let thumb = match download(shared, client, &key) {
            Ok(Some(bytes)) => {
                let raw_bytes = bytes.len();
                let thumb = shared.cache.store(&key.0, &key.1, &bytes);
                tracing::info!(
                    target: "hex::slippy",
                    ts = %key.0,
                    file = %key.1,
                    source = "download",
                    raw_bytes,
                    thumb_bytes = thumb.len(),
                    ms = started.elapsed().as_millis() as u64,
                    "Slippy thumbnail ready"
                );
                Some(thumb)
            }
            Ok(None) => None,
            Err(()) => return,
        };
        shared.update(|state| state.thumbs.insert(key, thumb));
    }
}

/// Downloads the open preview's full-size image when this Mac has none.
fn preview(shared: &Shared, client: &Client) {
    let Some((key, None)) = shared.state.lock().unwrap().preview.clone() else {
        return;
    };
    let Ok(Some(bytes)) = download(shared, client, &key) else {
        return;
    };
    let thumb = shared.cache.store(&key.0, &key.1, &bytes);
    let full = Arc::<[u8]>::from(bytes);
    shared.update(|state| {
        if state.preview.as_ref().is_some_and(|(open, _)| *open == key) {
            state.preview = Some((key.clone(), Some(full)));
        }
        if !state.thumbs.entries.contains_key(&key) {
            state.thumbs.insert(key, Some(thumb));
        }
    });
}

/// An attachment's bytes: `Ok(None)` when it won't load (not found, not an
/// image), `Err` for a failure worth trying again.
fn download(shared: &Shared, client: &Client, key: &FileKey) -> Result<Option<Vec<u8>>, ()> {
    let bytes = match client.file(&key.0, &key.1) {
        Ok((mime, bytes)) => (mime.starts_with("image/") && !bytes.is_empty()).then_some(bytes),
        Err(Error::Auth(AuthError::Refresh)) => {
            report(shared, Error::Auth(AuthError::Refresh));
            return Err(());
        }
        Err(Error::Rejected(status)) if status < 500 && status != 429 => None,
        Err(Error::Response) => None,
        Err(_) => return Err(()),
    };
    if bytes.is_none() {
        tracing::warn!(target: "hex::slippy", ts = %key.0, file = %key.1, "Slippy attachment unavailable");
    }
    Ok(bytes)
}

fn upsert(state: &mut State, task: Task) {
    if state
        .tasks
        .get(&task.ts)
        .is_none_or(|previous| task.seq >= previous.seq)
    {
        state.tasks.insert(task.ts.clone(), task);
    }
}

enum Cursor {
    Unseeded,
    Saved(Progress),
    Steady(Progress),
}

impl Cursor {
    fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path)
            .ok()
            .and_then(|value| value.trim().parse().ok())
        {
            Some(seq) => Self::Saved(Progress::new(seq)),
            None => Self::Unseeded,
        }
    }
}

#[derive(Clone, Copy)]
struct Progress {
    seq: u64,
    /// The finished task whose detail lagged the list, and how many polls.
    lagging: Option<(u64, u8)>,
}

impl Progress {
    fn new(seq: u64) -> Self {
        Self { seq, lagging: None }
    }

    /// Whether to wait another poll for `seq`'s detail to catch up.
    fn wait_for(&mut self, seq: u64) -> bool {
        let tries = match self.lagging {
            Some((lagging, tries)) if lagging == seq => tries.saturating_add(1),
            _ => 1,
        };
        self.lagging = Some((seq, tries));
        tries <= LAG_RETRIES
    }

    fn save(&mut self, path: &Path, seq: u64) -> Result<(), Error> {
        crate::durable::write_private(path, seq.to_string().as_bytes())
            .map_err(|_| Error::Unavailable)?;
        self.seq = seq;
        Ok(())
    }
}

fn poll(shared: &Shared, client: &Client, path: &Path, cursor: &mut Cursor) {
    if shared.cancelled.load(Ordering::Acquire) || shared.state.lock().unwrap().needs_reauth {
        return;
    }
    if let Err(error) = sync(shared, client, path, cursor) {
        report(shared, error);
    }
}

fn sync(shared: &Shared, client: &Client, path: &Path, cursor: &mut Cursor) -> Result<(), Error> {
    let progress = match cursor {
        Cursor::Unseeded => {
            let full = client.list(0, LIMIT)?;
            let mut progress = Progress::new(full.cursor);
            progress.save(path, full.cursor)?;
            load_history(shared, full.tasks);
            progress
        }
        Cursor::Saved(progress) => {
            let full = client.list(0, LIMIT)?;
            if progress.seq == full.cursor {
                load_history(shared, full.tasks);
            } else if progress.seq == 0 {
                delta(shared, client, path, progress, Some(full))?;
            } else {
                delta(shared, client, path, progress, None)?;
                // Leave changes deferred by delta out of the initial history.
                let seen = progress.seq;
                load_history(
                    shared,
                    full.tasks.into_iter().filter(|task| task.seq <= seen),
                );
            }
            *progress
        }
        Cursor::Steady(progress) => return delta(shared, client, path, progress, None),
    };
    *cursor = Cursor::Steady(progress);
    Ok(())
}

fn load_history(shared: &Shared, tasks: impl IntoIterator<Item = Task>) {
    shared.update(|state| {
        for task in tasks {
            upsert(state, task);
        }
        state.error = None;
    });
}

fn delta(
    shared: &Shared,
    client: &Client,
    path: &Path,
    progress: &mut Progress,
    mut snapshot: Option<Listed>,
) -> Result<(), Error> {
    loop {
        let mut since = progress.seq;
        let mut page = match snapshot.take() {
            Some(page) => page,
            None => client.list(since, LIMIT)?,
        };
        if since == 0 && page.tasks.len() == usize::from(LIMIT) {
            // Since zero serves a capped snapshot. Sequence 1 can only be
            // the first queued task, so start the paginated scan after it.
            since = 1;
            let first_task = page.tasks.into_iter().find(|task| task.seq == since);
            page = client.list(since, LIMIT)?;
            if let Some(task) = first_task {
                shared.update(|state| upsert(state, task));
            }
        }
        let more = since > 0 && page.tasks.len() == usize::from(LIMIT) && page.cursor > since;
        page.tasks.sort_by_key(|task| task.seq);
        for task in page.tasks {
            let finished = task.seq > since && !task.status.active();
            if finished && settle(shared, client, progress, &task)? == Settled::Lagging {
                // Neither the task nor the cursor moves: the next poll
                // sees the same change again.
                return Ok(());
            }
            let seq = task.seq;
            shared.update(|state| upsert(state, task));
            if finished {
                progress.save(path, seq)?;
            }
        }
        if page.cursor > progress.seq {
            progress.save(path, page.cursor)?;
        }
        shared.update(|state| state.error = None);
        if !more {
            return Ok(());
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Settled {
    Handled,
    Lagging,
}

/// Notifies for a finished (or waiting) `task` the list returned with a
/// newer `seq`: when its status changed, or when Slippy answered again. A
/// change to the title alone (the server naming the thread) notifies
/// nothing.
fn settle(
    shared: &Shared,
    client: &Client,
    progress: &mut Progress,
    task: &Task,
) -> Result<Settled, Error> {
    let previous = shared.state.lock().unwrap().tasks.get(&task.ts).cloned();
    if previous
        .as_ref()
        .is_some_and(|previous| previous.seq >= task.seq)
    {
        return Ok(Settled::Handled);
    }
    let detail = match client.detail(&task.ts) {
        Ok(detail) => detail,
        Err(Error::Rejected(404)) => return Ok(Settled::Handled),
        Err(error) => return Err(error),
    };
    if shared.cancelled.load(Ordering::Acquire) {
        return Err(Error::Unavailable);
    }
    if detail.task.seq < task.seq && progress.wait_for(task.seq) {
        return Ok(Settled::Lagging);
    }
    if detail.task.seq > task.seq && detail.task.status != task.status {
        // Already moved on: that change's own seq notifies.
        return Ok(Settled::Handled);
    }
    let reply = detail
        .messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant);
    let status_changed = previous
        .as_ref()
        .is_none_or(|previous| previous.status != task.status);
    shared.cache.store_detail(&detail);
    let local_thumbs = shared.cache.local_thumbs(&detail);
    let mut notify = status_changed;
    shared.update(|state| {
        remember(state, detail.clone());
        for (key, thumb) in local_thumbs {
            state.thumbs.insert(key, Some(thumb));
        }
        if let Some(reply) = reply {
            let answered = match state.replies.get(&task.ts) {
                Some(seen) => *seen != reply.ts,
                // Not checked since this sign-in: skip a title-only rename on an
                // already-loaded task; otherwise treat an unseen reply as new.
                None => previous
                    .as_ref()
                    .is_none_or(|previous| previous.title == task.title),
            };
            notify |= answered;
            state.replies.insert(task.ts.clone(), reply.ts.clone());
        }
        if !notify {
            return;
        }
        if state.selection.as_deref() != Some(&task.ts) || !shared.pane.load(Ordering::Acquire) {
            state.unread.insert(task.ts.clone());
        }
    });
    if notify {
        let body = reply
            .map(|message| message.text.chars().take(180).collect::<String>())
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| crate::desktop::i18n::tr(task.status.label()).into());
        tracing::info!(
            target: "hex::slippy",
            ts = %task.ts,
            seq = task.seq,
            status = task.status.label(),
            "posting Slippy notification"
        );
        super::notify::post(&task.ts, task.seq, "Slippy", &body);
    }
    progress.lagging = None;
    Ok(Settled::Handled)
}

#[cfg(test)]
impl Handle {
    /// A timed poll now, as the interval sends.
    pub(super) fn poll_now(&self) {
        let _ = self.send(Command::Poll);
    }
}

#[cfg(test)]
impl Worker {
    /// Stops the worker as sign-out does and waits for its thread.
    pub(super) fn stop_and_join(self) {
        self.handle.shared.cancelled.store(true, Ordering::Release);
        let _ = self.handle.commands.send(Command::Stop);
        self.thread.join().unwrap();
    }
}

#[cfg(test)]
pub(super) fn stopping() -> bool {
    let service = SERVICE.lock().unwrap();
    service.worker.is_some()
        || service
            .stopping
            .as_ref()
            .is_some_and(|done| matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)))
}
