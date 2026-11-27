use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::client::{Attachment, Client, Detail, Error, Origin, Role, Task};
use crate::team_auth::{AuthError, Client as AuthClient};

const LIMIT: u8 = 100;
const FAST: Duration = Duration::from_secs(5);
const IDLE: Duration = Duration::from_secs(60);
/// Attachments kept in memory at once; the oldest goes first.
pub(super) const MAX_IMAGES: usize = 24;
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
/// Fetched attachments: the bytes, or `None` when the fetch failed for good
/// (not found, not an image, too large). Absent while it loads.
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
    images: ImageCache,
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
    pub images: Images,
}

enum Command {
    Stop,
    Refresh,
    Select,
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
            worker.handle.refresh();
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
            images: state.images.entries.clone(),
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
            let (name, fallback) = if origin == Origin::Screenshot {
                ("screenshot.png", "Screenshot")
            } else {
                ("annotation.png", "Annotation")
            };
            let text = if words.trim().is_empty() {
                fallback.into()
            } else {
                words
            };
            Ok((origin, text, vec![super::image::attachment(name, &bytes)?]))
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

    pub(crate) fn select(&self, ts: String) {
        self.shared.update(|state| {
            state.unread.remove(&ts);
            let same = state.selection.as_deref() == Some(&ts);
            if !same {
                state.detail = None;
                state.selection = Some(ts);
            }
        });
        let _ = self.send(Command::Select);
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
                let _ = std::fs::remove_file(cursor_path);
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
    poll(shared, client, path, &mut cursor);
    while !shared.cancelled.load(Ordering::Acquire) {
        let command = match commands.recv_timeout(shared.interval()) {
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => Command::Refresh,
        };
        if shared.cancelled.load(Ordering::Acquire) || shared.state.lock().unwrap().needs_reauth {
            return;
        }
        match command {
            Command::Stop => return,
            Command::Refresh => {
                poll(shared, client, path, &mut cursor);
                detail(shared, client);
            }
            Command::Select => detail(shared, client),
            Command::Submit(prepare, answer) => {
                let result = prepare().and_then(|(origin, text, files)| {
                    if shared.cancelled.load(Ordering::Acquire) {
                        return Err(Error::Unavailable);
                    }
                    client
                        .create(origin, &text, &files)
                        .map(|created| created.task)
                });
                finish(shared, result, answer);
                poll(shared, client, path, &mut cursor);
            }
            Command::Reply(ts, text, answer) => {
                let result = client.reply(&ts, &text).map(|reply| reply.task);
                finish(shared, result, answer);
                poll(shared, client, path, &mut cursor);
                detail(shared, client);
            }
        }
    }
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
        state.needs_reauth |= error == Error::Auth(AuthError::Refresh);
    });
}

fn detail(shared: &Shared, client: &Client) {
    if !shared.pane.load(Ordering::Acquire) {
        return;
    }
    let Some(ts) = shared.state.lock().unwrap().selection.clone() else {
        return;
    };
    let result = client.detail(&ts);
    if result == Err(Error::Auth(AuthError::Refresh)) {
        report(shared, Error::Auth(AuthError::Refresh));
    }
    let loaded = result.as_ref().ok().cloned();
    shared.update(|state| {
        if state.selection.as_deref() == Some(&ts) {
            state.detail = Some(result);
        }
    });
    if let Some(detail) = loaded {
        images(shared, client, &detail);
    }
}

/// Fetches the selected thread's image attachments not yet in memory. A
/// transient failure leaves the image loading for the next detail; one that
/// won't change (not found, not an image) shows the file's name instead.
fn images(shared: &Shared, client: &Client, detail: &Detail) {
    let wanted = detail
        .messages
        .iter()
        .flat_map(|message| &message.files)
        .filter(|file| file.mime_type.starts_with("image/"))
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
            .images
            .entries
            .contains_key(&key)
        {
            continue;
        }
        let bytes = match client.file(&key.0, &key.1) {
            Ok((mime, bytes)) => {
                (mime.starts_with("image/") && !bytes.is_empty()).then(|| Arc::from(bytes))
            }
            Err(Error::Auth(AuthError::Refresh)) => {
                report(shared, Error::Auth(AuthError::Refresh));
                return;
            }
            Err(Error::Rejected(status)) if status < 500 && status != 429 => None,
            Err(Error::Response) => None,
            Err(_) => return,
        };
        if bytes.is_none() {
            tracing::warn!(target: "hex::slippy", ts = %key.0, file = %key.1, "Slippy attachment unavailable");
        }
        shared.update(|state| state.images.insert(key, bytes));
    }
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

struct Cursor {
    last_notified_seq: Option<u64>,
    loaded: bool,
    /// The finished task whose detail lagged the list, and how many polls.
    lagging: Option<(u64, u8)>,
}

impl Cursor {
    fn load(path: &Path) -> Self {
        Self {
            last_notified_seq: std::fs::read_to_string(path)
                .ok()
                .and_then(|value| value.trim().parse().ok()),
            loaded: false,
            lagging: None,
        }
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

    fn save(&mut self, path: &Path, seq: u64) -> bool {
        if crate::durable::write_private(path, seq.to_string().as_bytes()).is_err() {
            return false;
        }
        self.last_notified_seq = Some(seq);
        true
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
    if !cursor.loaded {
        let full = client.list(0, LIMIT)?;
        if cursor.last_notified_seq.is_none() {
            cursor.save(path, full.cursor);
        }
        // A saved cursor may have unseen completions. Apply the delta first
        // so the initial history load doesn't swallow their status changes.
        if cursor.last_notified_seq == Some(0) || cursor.last_notified_seq == Some(full.cursor) {
            shared.update(|state| {
                for task in full.tasks {
                    upsert(state, task);
                }
                state.error = None;
            });
            cursor.loaded = true;
            return Ok(());
        }
        if cursor.last_notified_seq.is_none() {
            return Err(Error::Unavailable);
        }
        delta(shared, client, path, cursor)?;
        // What the delta left for a later poll stays out, or that poll
        // would see no change to notify.
        let seen = cursor.last_notified_seq.unwrap_or(0);
        shared.update(|state| {
            for task in full.tasks.into_iter().filter(|task| task.seq <= seen) {
                upsert(state, task);
            }
        });
        cursor.loaded = true;
        return Ok(());
    }
    delta(shared, client, path, cursor)
}

fn delta(shared: &Shared, client: &Client, path: &Path, cursor: &mut Cursor) -> Result<(), Error> {
    loop {
        let since = cursor.last_notified_seq.unwrap_or(0);
        let mut page = client.list(since, LIMIT)?;
        let more = since > 0 && page.tasks.len() == usize::from(LIMIT) && page.cursor > since;
        page.tasks.sort_by_key(|task| task.seq);
        for task in page.tasks {
            let finished = task.seq > since && !task.status.active();
            if finished && settle(shared, client, cursor, &task)? == Settled::Lagging {
                // Neither the task nor the cursor moves: the next poll
                // sees the same change again.
                return Ok(());
            }
            let seq = task.seq;
            shared.update(|state| upsert(state, task));
            if finished && !cursor.save(path, seq) {
                return Err(Error::Unavailable);
            }
        }
        if page.cursor > cursor.last_notified_seq.unwrap_or(0) && !cursor.save(path, page.cursor) {
            return Err(Error::Unavailable);
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
    cursor: &mut Cursor,
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
    if detail.task.seq < task.seq && cursor.wait_for(task.seq) {
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
    let mut notify = status_changed;
    shared.update(|state| {
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
    cursor.lagging = None;
    Ok(Settled::Handled)
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
