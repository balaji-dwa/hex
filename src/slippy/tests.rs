//! The Slippy client against a loopback `HEX_TEAM_AUTH_ORIGIN`: the four
//! `/api/hex/tasks*` endpoints as the wire contract names them, the
//! notification cursor, sign-out, and the attachments it prepares.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::cache::DiskCache;
use super::client::{Attachment, Client, Error, Origin, Status, base64, valid_ts};
use super::image::Kind;
use super::sync::{self, Worker};
use crate::team_auth::{AuthError, Client as AuthClient, test_client};

const TS: &str = "991795815721.000001";
const MEMBER: &str = "member-sub";

#[derive(Clone, Debug)]
struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

type Reply = (u16, Vec<(&'static str, String)>, Value);
type Handler = Box<dyn FnMut(&Request) -> Reply + Send>;

/// A one-connection-at-a-time HTTP server that records every request.
struct Mock {
    origin: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Mock {
    fn start(handler: impl FnMut(&Request) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (server_requests, server_stop) = (requests.clone(), stop.clone());
        let mut handler: Handler = Box::new(handler);
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if server_stop.load(Ordering::Acquire) {
                    break;
                }
                let mut stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let Some(request) = read(&mut stream) else {
                    continue;
                };
                server_requests.lock().unwrap().push(request.clone());
                let (status, headers, body) = handler(&request);
                respond(&mut stream, status, &headers, &body);
            }
        });
        Self {
            origin,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    fn client(&self, dir: &Path) -> AuthClient {
        test_client(dir.join("session.json"), &self.origin, MEMBER, "access-0")
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.origin.trim_start_matches("http://"));
        let result = self.thread.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}

fn read(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(&mut *stream);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_ascii_whitespace();
    let method = parts.next()?.to_owned();
    let url = url::Url::parse(&format!("http://mock{}", parts.next()?)).ok()?;
    let mut headers = HashMap::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 || line == "\r\n" {
            break;
        }
        let (name, value) = line.split_once(':')?;
        headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
    }
    let size = headers
        .get("content-length")
        .map_or(0, |length| length.parse().unwrap());
    let mut body = vec![0; size];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path: url.path().to_owned(),
        query: url.query_pairs().into_owned().collect(),
        headers,
        body,
    })
}

/// Writes `body` as JSON, or, under a `Content-Type` header, an array of
/// byte values as those raw bytes.
fn respond(stream: &mut TcpStream, status: u16, headers: &[(&str, String)], body: &Value) {
    let raw = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"));
    let (content_type, data) = match (raw, body) {
        (Some((_, kind)), Value::Array(bytes)) => (
            kind.as_str(),
            bytes
                .iter()
                .map(|byte| byte.as_u64().unwrap() as u8)
                .collect(),
        ),
        _ => ("application/json", body.to_string().into_bytes()),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} Mock\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: private, no-store\r\n",
        data.len()
    );
    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("content-type") {
            let _ = write!(stream, "{name}: {value}\r\n");
        }
    }
    let _ = write!(stream, "\r\n");
    let _ = stream.write_all(&data);
}

fn raw(content_type: &str, bytes: &[u8]) -> Reply {
    (
        200,
        vec![("Content-Type", content_type.into())],
        Value::Array(bytes.iter().map(|byte| json!(byte)).collect()),
    )
}

fn task(ts: &str, status: &str, seq: u64) -> Value {
    json!({
        "ts": ts,
        "origin": "dictation",
        "title": "Draft the renewal email",
        "status": status,
        "seq": seq,
        "createdAt": 1_700_000_000_000_u64,
        "updatedAt": 1_700_000_000_000_u64 + seq,
    })
}

fn ok(body: Value) -> Reply {
    (200, Vec::new(), body)
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes).unwrap();
        let name: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("hex-slippy-{name}"));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

fn idempotency_key_is_valid(key: &str) -> bool {
    let bytes = key.as_bytes();
    (1..=128).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[test]
fn the_four_endpoints_match_the_wire_contract() {
    let dir = TempDir::new();
    let server = Mock::start(
        |request| match (request.method.as_str(), request.path.as_str()) {
            ("POST", "/api/hex/tasks") => (
                202,
                Vec::new(),
                json!({"task": task(TS, "queued", 1), "deduplicated": false}),
            ),
            ("GET", "/api/hex/tasks") => ok(json!({"cursor": 3, "tasks": [task(TS, "done", 3)]})),
            ("GET", path) if path == format!("/api/hex/tasks/{TS}") => ok(json!({
                "task": task(TS, "waiting", 2),
                "messages": [
                    {"ts": TS, "role": "user", "text": "Draft it", "files": [{"id": "f1", "name": "screenshot.png", "mimeType": "image/png"}]},
                    {"ts": "991795815721.000002", "role": "assistant", "text": "Here is a draft.", "files": []},
                ],
            })),
            ("POST", path) if path == format!("/api/hex/tasks/{TS}/messages") => (
                202,
                Vec::new(),
                json!({"task": task(TS, "running", 4), "messageTs": "991795815721.000003", "deduplicated": false}),
            ),
            _ => (404, Vec::new(), json!({"error": "Not found"})),
        },
    );
    let auth = server.client(&dir.0);
    let cancel = AtomicBool::new(false);
    let client = Client {
        auth: &auth,
        member: MEMBER,
        cancel: &cancel,
    };
    let files = [Attachment {
        name: "screenshot.png",
        mime_type: "image/png",
        base64: base64(b"png"),
        raw: b"png".to_vec(),
    }];

    let created = client
        .create(Origin::Screenshot, "  Look at this  ", &files)
        .unwrap();
    assert_eq!(created.task.ts, TS);
    assert_eq!(created.task.status, Status::Queued);
    assert!(!created.deduplicated);
    let listed = client.list(0, 50).unwrap();
    assert_eq!(listed.cursor, 3);
    assert_eq!(listed.tasks[0].status, Status::Done);
    let detail = client.detail(TS).unwrap();
    assert_eq!(detail.task.status, Status::Waiting);
    assert_eq!(detail.messages[0].files[0].mime_type, "image/png");
    assert_eq!(detail.messages[1].text, "Here is a draft.");
    let replied = client.reply(TS, "Send it").unwrap();
    assert_eq!(replied.task.status, Status::Running);
    assert_eq!(replied.message_ts, "991795815721.000003");
    assert!(!replied.deduplicated);

    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    for request in &requests {
        assert_eq!(request.headers["authorization"], "Bearer access-0");
        assert!(!request.headers.contains_key("cf-access-jwt-assertion"));
        if request.method == "POST" {
            assert_eq!(request.headers["content-type"], "application/json");
        }
    }
    let create = requests[0].json();
    assert_eq!(keys(&create), ["files", "idempotencyKey", "origin", "text"]);
    assert_eq!(create["origin"], "screenshot");
    assert_eq!(create["text"], "Look at this");
    assert!(idempotency_key_is_valid(
        create["idempotencyKey"].as_str().unwrap()
    ));
    assert_eq!(keys(&create["files"][0]), ["base64", "mimeType", "name"]);
    assert_eq!(create["files"][0]["base64"], "cG5n");
    assert_eq!(requests[1].path, "/api/hex/tasks");
    assert_eq!(requests[1].query["since"], "0");
    assert_eq!(requests[1].query["limit"], "50");
    assert_eq!(requests[2].path, format!("/api/hex/tasks/{TS}"));
    let reply = requests[3].json();
    assert_eq!(keys(&reply), ["files", "idempotencyKey", "text"]);
    assert_eq!(reply["files"], json!([]));
    assert_ne!(reply["idempotencyKey"], create["idempotencyKey"]);
}

#[test]
fn a_rejected_bearer_refreshes_once_and_resends_the_same_idempotency_key() {
    let dir = TempDir::new();
    let server = Mock::start(
        |request| match (request.method.as_str(), request.path.as_str()) {
            ("POST", "/cdn-cgi/access/oauth/token") => ok(json!({
                "access_token": "access-1", "refresh_token": "refresh-1", "expires_in": 3600,
            })),
            ("POST", "/api/hex/tasks") if request.headers["authorization"] == "Bearer access-0" => {
                (
                    302,
                    vec![(
                        "Location",
                        "https://team.cloudflareaccess.com/cdn-cgi/access/login/hex.example".into(),
                    )],
                    json!({}),
                )
            }
            ("POST", "/api/hex/tasks") => (
                200,
                Vec::new(),
                json!({"task": task(TS, "queued", 1), "deduplicated": true}),
            ),
            _ => (404, Vec::new(), json!({"error": "Not found"})),
        },
    );
    let auth = server.client(&dir.0);
    let cancel = AtomicBool::new(false);
    let client = Client {
        auth: &auth,
        member: MEMBER,
        cancel: &cancel,
    };
    assert!(
        client
            .create(Origin::Dictation, "hello", &[])
            .unwrap()
            .deduplicated
    );
    let posts: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/api/hex/tasks")
        .collect();
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[0].body, posts[1].body);
    assert_eq!(posts[1].headers["authorization"], "Bearer access-1");
}

#[test]
fn rejected_requests_map_to_their_errors_without_bad_paths() {
    let dir = TempDir::new();
    let server = Mock::start(|request| match request.path.as_str() {
        "/api/hex/tasks" => (429, Vec::new(), json!({"error": "Too many open tasks"})),
        _ => (404, Vec::new(), json!({"error": "Not found"})),
    });
    let auth = server.client(&dir.0);
    let cancel = AtomicBool::new(false);
    let client = Client {
        auth: &auth,
        member: MEMBER,
        cancel: &cancel,
    };
    assert_eq!(
        client.create(Origin::Dictation, "hello", &[]).err(),
        Some(Error::Rejected(429))
    );
    assert_eq!(client.detail(TS).err(), Some(Error::Rejected(404)));
    // Never sent: the text is empty or too long, or the ts isn't one.
    assert_eq!(
        client.create(Origin::Dictation, "   ", &[]).err(),
        Some(Error::Text)
    );
    assert_eq!(
        client
            .create(Origin::Dictation, &"x".repeat(12_001), &[])
            .err(),
        Some(Error::Text)
    );
    assert_eq!(client.detail("../whoami").err(), Some(Error::Rejected(404)));
    assert_eq!(server.requests().len(), 2);
    assert!(valid_ts("1700000000.000001"));
    assert!(!valid_ts("170000000.000001"));
    assert!(!valid_ts("1700000000.00001"));
}

/// The list the mock serves: since 0, the whole history and its cursor;
/// after that, what changed since.
#[derive(Default)]
struct History {
    tasks: Vec<Value>,
}

impl History {
    fn list(&self, since: u64) -> Value {
        let seq = |task: &Value| task["seq"].as_u64().unwrap();
        if since == 0 {
            let cursor = self.tasks.iter().map(seq).max().unwrap_or(0);
            json!({"cursor": cursor, "tasks": self.tasks})
        } else {
            let tasks: Vec<_> = self
                .tasks
                .iter()
                .filter(|task| seq(task) > since)
                .cloned()
                .collect();
            let cursor = tasks.iter().map(seq).max().unwrap_or(since);
            json!({"cursor": cursor, "tasks": tasks})
        }
    }
}

fn history_server(history: Arc<Mutex<History>>) -> Mock {
    Mock::start(move |request| {
        let history = history.lock().unwrap();
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/api/hex/tasks") => ok(history.list(request.query["since"].parse().unwrap())),
            ("GET", path) => {
                let ts = path.trim_start_matches("/api/hex/tasks/");
                let task = history.tasks.iter().find(|task| task["ts"] == ts).unwrap();
                ok(json!({
                    "task": task,
                    "messages": [{"ts": ts, "role": "assistant", "text": "Drafted the reply.", "files": []}],
                }))
            }
            _ => (404, Vec::new(), json!({"error": "Not found"})),
        }
    })
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn posted(ts: &str) -> Vec<(String, String)> {
    super::notify::POSTED
        .lock()
        .unwrap()
        .iter()
        .filter(|(identifier, _)| identifier.contains(ts))
        .cloned()
        .collect()
}

#[test]
fn the_cursor_is_seeded_privately_and_each_completion_notifies_once() {
    let dir = TempDir::new();
    let (old, new) = ("991795815722.000001", "991795815722.000002");
    let history = Arc::new(Mutex::new(History {
        tasks: vec![task(old, "done", 5)],
    }));
    let server = history_server(history.clone());
    let auth = server.client(&dir.0);
    let cursor = auth.cursor_path(MEMBER);
    let worker = Worker::start(auth, MEMBER, false, Arc::new(|| {})).unwrap();
    wait_until("the first load", || {
        worker.handle.snapshot().tasks.len() == 1
    });
    // First sign-in: nothing finished before it notifies.
    assert_eq!(std::fs::read_to_string(&cursor).unwrap(), "5");
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&cursor).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(posted(old).is_empty());

    history.lock().unwrap().tasks.push(task(new, "running", 6));
    worker.handle.refresh();
    wait_until("the running task", || {
        worker.handle.snapshot().tasks.len() == 2
    });
    assert!(posted(new).is_empty());
    history.lock().unwrap().tasks[1] = task(new, "done", 7);
    worker.handle.refresh();
    wait_until("the completion", || !posted(new).is_empty());
    worker.handle.refresh();
    worker.handle.refresh();
    wait_until("the cursor", || {
        std::fs::read_to_string(&cursor).is_ok_and(|seq| seq == "7")
    });
    let snapshot = worker.handle.snapshot();
    assert!(snapshot.unread.contains(new));
    assert_eq!(
        posted(new),
        [(format!("hex.slippy.{new}.7"), "Drafted the reply.".into())]
    );
    // The thread is still alive: another refresh is answered, not queued.
    let version = snapshot.version;
    worker.handle.refresh();
    wait_until("another poll", || {
        worker.handle.snapshot().version > version
    });
    worker.stop_and_join();
    // Stopping (sign-out) drops the cursor with the session.
    assert!(!cursor.exists());

    // A restart with a saved cursor reloads history without notifying again,
    // and notifies what finished while Hex was closed.
    let later = "991795815722.000003";
    std::fs::write(&cursor, "7").unwrap();
    history.lock().unwrap().tasks.push(task(later, "failed", 8));
    let auth = server.client(&dir.0);
    let worker = Worker::start(auth, MEMBER, false, Arc::new(|| {})).unwrap();
    wait_until("the reload", || worker.handle.snapshot().tasks.len() == 3);
    wait_until("the cursor", || {
        std::fs::read_to_string(&cursor).is_ok_and(|seq| seq == "8")
    });
    assert_eq!(
        posted(later),
        [(format!("hex.slippy.{later}.8"), "Drafted the reply.".into())]
    );
    assert_eq!(posted(new).len(), 1);
    assert!(posted(old).is_empty());
    worker.stop_and_join();
}

#[test]
fn sign_out_stops_the_worker_and_clears_its_state() {
    let dir = TempDir::new();
    let ts = "991795815723.000001";
    let history = Arc::new(Mutex::new(History {
        tasks: vec![task(ts, "waiting", 1)],
    }));
    let server = history_server(history);
    let auth = server.client(&dir.0);
    let cursor = auth.cursor_path(MEMBER);
    let auth = Mutex::new(Some(auth));
    let take = || auth.lock().unwrap().take().ok_or(AuthError::Storage);
    assert!(!super::active());
    assert_eq!(super::handle().err(), Some(Error::Unavailable));
    sync::reconcile_with(Some(MEMBER), true, || {}, take);
    assert!(super::active());
    wait_until("the first load", || super::snapshot().tasks.len() == 1);
    assert!(cursor.exists());

    super::sign_out();
    assert!(!super::active());
    assert!(super::snapshot().tasks.is_empty());
    assert_eq!(super::handle().err(), Some(Error::Unavailable));
    sync::reconcile_with(None, false, || {}, || Err(AuthError::Storage));
    wait_until("the worker to stop", || !sync::stopping());
    assert!(!cursor.exists());
    // Signed out, nothing starts again.
    sync::reconcile_with(None, false, || {}, || panic!("signed out"));
    assert!(!super::active());
}

fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            rgb.extend(pixel(x, y));
        }
    }
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&rgb)
        .unwrap();
    bytes
}

fn size_of(bytes: &[u8]) -> (u32, u32) {
    let reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let info = reader.info();
    (info.width, info.height)
}

#[test]
fn a_5k_screenshot_is_shrunk_to_a_2560_long_edge() {
    let source = png(5120, 2880, |x, y| [(x / 20) as u8, (y / 12) as u8, 128]);
    let scaled = super::image::downscale(&source).unwrap();
    assert_eq!(size_of(&scaled), (2560, 1440));
    let small = png(1200, 800, |x, _| [x as u8, 0, 0]);
    assert_eq!(super::image::downscale(&small).unwrap(), small);
    assert!(super::image::downscale(b"not a png").is_err());
}

#[test]
fn a_noisy_image_is_shrunk_until_it_fits_the_upload() {
    let mut state = 0x2545_f491_u32;
    let mut noise = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state.to_le_bytes()
    };
    let mut pixels = Vec::new();
    for _ in 0..(2600 * 1600) {
        let [r, g, b, _] = noise();
        pixels.push([r, g, b]);
    }
    let source = png(2600, 1600, |x, y| pixels[(y * 2600 + x) as usize]);
    assert!(source.len() > 4 * 1024 * 1024);
    let scaled = super::image::downscale(&source).unwrap();
    assert!(scaled.len() <= 4 * 1024 * 1024);
    let (width, height) = size_of(&scaled);
    assert!(width < 2560 && height < 1600);
    // ImageIO's JPEG of it stays well under the 4 MiB upload.
    let attachment = super::image::attachment(Kind::Screenshot, &source).unwrap();
    assert!(attachment.raw.len() <= 4 * 1024 * 1024);
    assert_eq!(attachment.mime_type, "image/jpeg");
    assert_eq!(attachment.name, "screenshot.jpg");
    assert!(jpeg_size(&attachment.raw).0 <= 2048);
}

#[test]
fn an_annotation_bundle_sends_its_contact_sheet_and_words() {
    let dir = TempDir::new();
    let bundle = dir.0.join("take.hexannotation");
    std::fs::create_dir(&bundle).unwrap();
    std::fs::write(
        bundle.join("sheet.png"),
        png(3000, 1000, |x, _| [x as u8, 0, 0]),
    )
    .unwrap();
    let manifest = |words: &str| -> scrawl_capture::model::ScrawlBundle {
        serde_json::from_value(json!({
            "version": 1,
            "id": "take",
            "createdAtMs": 1_700_000_000_000_u64,
            "durationMs": 1000,
            "target": {"mode": "display", "displayId": 1, "frame": {"x": 0.0, "y": 0.0, "width": 3000.0, "height": 1000.0, "scale": 1.0}},
            "transcript": {"source": "mic", "text": words, "segments": []},
            "strokes": [],
            "marks": [],
            "events": [],
            "keyframes": [],
            "contactSheet": "sheet.png",
        }))
        .unwrap()
    };
    scrawl_capture::bundle::publish(&bundle, &manifest("Fix this button")).unwrap();
    let (text, attachment) = super::bundle::read(&bundle).unwrap();
    assert_eq!(text, "Fix this button");
    assert_eq!(attachment.name, "annotation.jpg");
    assert_eq!(attachment.mime_type, "image/jpeg");
    assert_eq!(jpeg_size(&attachment.raw), (2048, 683));
    scrawl_capture::bundle::publish(&bundle, &manifest("  ")).unwrap();
    assert_eq!(super::bundle::read(&bundle).unwrap().0, "Annotation");
    std::fs::remove_file(bundle.join("sheet.png")).unwrap();
    assert_eq!(super::bundle::read(&bundle).err(), Some(Error::Image));
}

/// A server whose tasks the test edits, each detail answering with the
/// task's own messages.
#[derive(Default)]
struct Threads {
    tasks: Vec<Value>,
    messages: HashMap<String, Vec<Value>>,
    /// Details answer with this task state instead, as a lagging read.
    stale: Option<Value>,
    files: HashMap<String, (&'static str, Vec<u8>)>,
    /// The `ts` a create answers with.
    created: Option<String>,
}

fn threads_server(threads: Arc<Mutex<Threads>>) -> Mock {
    Mock::start(move |request| {
        let mut threads = threads.lock().unwrap();
        let path = request.path.trim_start_matches("/api/hex/tasks");
        match (
            request.method.as_str(),
            path.split('/').collect::<Vec<_>>().as_slice(),
        ) {
            ("GET", [""]) => ok(History {
                tasks: threads.tasks.clone(),
            }
            .list(request.query["since"].parse().unwrap())),
            ("GET", ["", ts]) => {
                let task = threads.tasks.iter().find(|task| task["ts"] == *ts).unwrap();
                ok(json!({
                    "task": threads.stale.as_ref().unwrap_or(task),
                    "messages": threads.messages.get(*ts).cloned().unwrap_or_default(),
                }))
            }
            // A new task under `created`: its message names each upload `U<n>`.
            ("POST", [""]) => {
                let ts = threads.created.clone().unwrap();
                let body = request.json();
                let files: Vec<Value> = body["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .map(|(index, file)| {
                        json!({"id": format!("U{}", index + 1), "name": file["name"], "mimeType": file["mimeType"]})
                    })
                    .collect();
                let seq = threads.tasks.len() as u64 + 1;
                let created = task(&ts, "queued", seq);
                threads.tasks.push(created.clone());
                threads.messages.insert(
                    ts.clone(),
                    vec![json!({"ts": ts, "role": "user", "text": body["text"], "files": files})],
                );
                (
                    202,
                    Vec::new(),
                    json!({"task": created, "deduplicated": false}),
                )
            }
            ("GET", ["", _, "files", id]) => match threads.files.get(*id) {
                Some((kind, bytes)) => raw(kind, bytes),
                None => (404, Vec::new(), json!({"error": "Not found"})),
            },
            _ => (404, Vec::new(), json!({"error": "Not found"})),
        }
    })
}

fn titled(ts: &str, status: &str, seq: u64, title: &str) -> Value {
    let mut task = task(ts, status, seq);
    task["title"] = json!(title);
    task
}

fn answer(ts: &str, text: &str) -> Value {
    json!({"ts": ts, "role": "assistant", "text": text, "files": []})
}

#[test]
fn a_new_title_alone_never_notifies_again_but_a_new_answer_does() {
    let dir = TempDir::new();
    let (done, waiting) = ("991795815724.000001", "991795815724.000002");
    let threads = Arc::new(Mutex::new(Threads {
        tasks: vec![task(done, "running", 1), task(waiting, "running", 2)],
        ..Threads::default()
    }));
    let server = threads_server(threads.clone());
    let worker = Worker::start(server.client(&dir.0), MEMBER, false, Arc::new(|| {})).unwrap();
    wait_until("the first load", || {
        worker.handle.snapshot().tasks.len() == 2
    });

    // Both finish: each notifies once.
    {
        let mut threads = threads.lock().unwrap();
        threads.tasks = vec![task(done, "done", 3), task(waiting, "waiting", 4)];
        threads
            .messages
            .insert(done.into(), vec![answer("991795815724.000010", "Sent it.")]);
        threads.messages.insert(
            waiting.into(),
            vec![answer("991795815724.000011", "Which account?")],
        );
    }
    worker.handle.refresh();
    wait_until("both notifications", || {
        posted(done).len() == 1 && posted(waiting).len() == 1
    });

    // The server names the finished thread: a new seq, same status and answer.
    threads.lock().unwrap().tasks[0] = titled(done, "done", 5, "Renewal email sent");
    worker.handle.refresh();
    wait_until("the new title", || {
        worker
            .handle
            .snapshot()
            .tasks
            .iter()
            .any(|task| task.title == "Renewal email sent")
    });
    assert_eq!(posted(done).len(), 1, "a title change notified");

    // Still waiting, but Slippy asked something new: that notifies.
    {
        let mut threads = threads.lock().unwrap();
        threads.tasks[1] = task(waiting, "waiting", 6);
        threads
            .messages
            .get_mut(waiting)
            .unwrap()
            .push(answer("991795815724.000012", "And which quarter?"));
    }
    worker.handle.refresh();
    wait_until("the second question", || posted(waiting).len() == 2);
    assert_eq!(
        posted(waiting)[1],
        (
            format!("hex.slippy.{waiting}.6"),
            "And which quarter?".into()
        )
    );
    assert_eq!(posted(done).len(), 1);
    worker.stop_and_join();
}

#[test]
fn a_detail_behind_the_list_holds_the_notification_until_it_catches_up() {
    let dir = TempDir::new();
    let ts = "991795815725.000001";
    let threads = Arc::new(Mutex::new(Threads {
        tasks: vec![task(ts, "running", 1)],
        ..Threads::default()
    }));
    let server = threads_server(threads.clone());
    let auth = server.client(&dir.0);
    let cursor = auth.cursor_path(MEMBER);
    let worker = Worker::start(auth, MEMBER, false, Arc::new(|| {})).unwrap();
    wait_until("the first load", || {
        worker.handle.snapshot().tasks.len() == 1
    });
    {
        let mut threads = threads.lock().unwrap();
        threads.tasks[0] = task(ts, "done", 2);
        threads.stale = Some(task(ts, "running", 1));
        threads
            .messages
            .insert(ts.into(), vec![answer("991795815725.000010", "Done.")]);
    }
    worker.handle.refresh();
    let details = || {
        server
            .requests()
            .iter()
            .filter(|request| request.path.ends_with(ts))
            .count()
    };
    wait_until("the stale detail", || details() >= 1);
    worker.handle.refresh();
    wait_until("another stale detail", || details() >= 2);
    assert!(posted(ts).is_empty());
    assert_eq!(std::fs::read_to_string(&cursor).unwrap(), "1");
    assert_eq!(worker.handle.snapshot().tasks[0].status, Status::Running);

    threads.lock().unwrap().stale = None;
    worker.handle.refresh();
    wait_until("the notification", || posted(ts).len() == 1);
    wait_until("the cursor", || {
        std::fs::read_to_string(&cursor).is_ok_and(|seq| seq == "2")
    });
    worker.stop_and_join();
}

fn image_threads(ts: &str, png: Vec<u8>) -> Threads {
    let file = |id: &str, name: &str, kind: &str| json!({"id": id, "name": name, "mimeType": kind});
    Threads {
        tasks: vec![task(ts, "done", 1)],
        messages: HashMap::from([(
            ts.to_owned(),
            vec![json!({
                "ts": ts,
                "role": "user",
                "text": "Fix this",
                "files": [
                    file("F1", "annotation.png", "image/png"),
                    file("F2", "notes.txt", "text/plain"),
                    file("F3", "gone.png", "image/png"),
                ],
            })],
        )]),
        files: HashMap::from([
            ("F1".to_owned(), ("image/png", png)),
            ("F2".to_owned(), ("text/plain", b"notes".to_vec())),
        ]),
        ..Threads::default()
    }
}

#[test]
fn the_file_endpoint_returns_an_attachments_bytes() {
    let dir = TempDir::new();
    let image = png(4, 4, |x, y| [x as u8 * 60, y as u8 * 60, 0]);
    let server = threads_server(Arc::new(Mutex::new(image_threads(TS, image.clone()))));
    let auth = server.client(&dir.0);
    let cancel = AtomicBool::new(false);
    let client = Client {
        auth: &auth,
        member: MEMBER,
        cancel: &cancel,
    };
    assert_eq!(
        client.file(TS, "F1").unwrap(),
        ("image/png".to_owned(), image)
    );
    assert_eq!(client.file(TS, "F3").err(), Some(Error::Rejected(404)));
    // Never sent: neither names one file of one task.
    assert_eq!(client.file(TS, "../F1").err(), Some(Error::Rejected(404)));
    assert_eq!(client.file("../x", "F1").err(), Some(Error::Rejected(404)));
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, format!("/api/hex/tasks/{TS}/files/F1"));
    assert_eq!(requests[0].headers["authorization"], "Bearer access-0");
}

#[test]
fn the_selected_threads_images_are_fetched_once_and_dropped_at_sign_out() {
    let dir = TempDir::new();
    let ts = "991795815726.000001";
    let image = png(4, 4, |x, _| [x as u8 * 60, 0, 0]);
    let server = threads_server(Arc::new(Mutex::new(image_threads(ts, image.clone()))));
    let auth = server.client(&dir.0);
    let cache_dir = auth.cache_dir(MEMBER);
    let auth = Mutex::new(Some(auth));
    let take = || auth.lock().unwrap().take().ok_or(AuthError::Storage);
    sync::reconcile_with(Some(MEMBER), true, || {}, take);
    wait_until("the first load", || super::snapshot().tasks.len() == 1);
    super::handle().unwrap().select(ts.into());
    let key = |id: &str| (ts.to_owned(), id.to_owned());
    wait_until("the images", || {
        let images = super::snapshot().thumbs;
        images.contains_key(&key("F1")) && images.contains_key(&key("F3"))
    });
    let images = super::snapshot().thumbs;
    // Its thumbnail: a small JPEG, the full image kept on disk.
    let thumb = images[&key("F1")].clone().unwrap();
    assert_eq!(&thumb[..2], [0xFF, 0xD8]);
    // The preview reads the full image from disk, not the server.
    super::handle().unwrap().open_preview(key("F1"));
    let opened = super::snapshot().preview_image.unwrap();
    assert_eq!((opened.0, &opened.1[..]), (key("F1"), image.as_slice()));
    super::handle().unwrap().close_preview();
    assert!(super::snapshot().preview_image.is_none());
    // Not found: the file's name shows instead, and it isn't asked again.
    assert_eq!(images[&key("F3")], None);
    // Not an image: never fetched.
    assert!(!images.contains_key(&key("F2")));
    super::handle().unwrap().refresh();
    super::handle().unwrap().refresh();
    let version = super::snapshot().version;
    wait_until("another detail", || super::snapshot().version > version);
    let fetched = |id: &str| {
        server
            .requests()
            .iter()
            .filter(|request| request.path.ends_with(&format!("/files/{id}")))
            .count()
    };
    assert_eq!((fetched("F1"), fetched("F2"), fetched("F3")), (1, 0, 1));

    super::sign_out();
    assert!(super::snapshot().thumbs.is_empty());
    sync::reconcile_with(None, false, || {}, || Err(AuthError::Storage));
    wait_until("the worker to stop", || !sync::stopping());
    // Signed out: this member's images are gone from disk too.
    assert!(!cache_dir.exists());
}

#[test]
fn the_image_cache_keeps_the_newest_attachments() {
    let mut cache = sync::ImageCache::default();
    for index in 0..sync::MAX_IMAGES + 3 {
        cache.insert(
            (TS.into(), format!("F{index}")),
            Some(Arc::from(&b"png"[..])),
        );
    }
    let entries = &cache.entries;
    assert_eq!(entries.len(), sync::MAX_IMAGES);
    assert!(!entries.contains_key(&(TS.into(), "F2".into())));
    assert!(entries.contains_key(&(TS.into(), "F3".into())));
}

/// A JPEG's pixel size, from its frame header.
fn jpeg_size(bytes: &[u8]) -> (u32, u32) {
    assert_eq!(&bytes[..2], [0xFF, 0xD8], "not a JPEG");
    let mut at = 2;
    while at + 9 < bytes.len() {
        assert_eq!(bytes[at], 0xFF);
        let marker = bytes[at + 1];
        let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
            let height = u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]);
            let width = u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]);
            return (width.into(), height.into());
        }
        at += 2 + len;
    }
    panic!("no JPEG frame");
}

/// A Retina screenshot's look: a flat sidebar, lines of dark "text" on a
/// light page, and a photo-like panel of soft noise.
fn screenshot(width: u32, height: u32) -> Vec<u8> {
    let mut state = 0x9e37_79b9_u32;
    let mut noise = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state & 0x07) as u8
    };
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let grain = noise();
            pixels.push(if x < width / 5 {
                [236, 236, 240]
            } else if x > width * 3 / 5 && y > height / 4 && y < height * 3 / 4 {
                [
                    (x * 160 / width) as u8 + grain,
                    (y * 160 / height) as u8 + grain,
                    120 + grain,
                ]
            } else if y % 32 < 10 && (x / 9) % 7 < 5 && (x + y) % 3 != 0 {
                [40, 40, 48]
            } else {
                [252, 252, 252]
            });
        }
    }
    png(width, height, |x, y| pixels[(y * width + x) as usize])
}

#[test]
fn uploads_are_jpegs_at_most_2048_pixels_and_smaller_than_the_png() {
    let source = screenshot(3024, 1964);
    let attachment = super::image::attachment(Kind::Screenshot, &source).unwrap();
    let jpeg = &attachment.raw;
    eprintln!(
        "3024x1964 screenshot: PNG {} bytes -> JPEG {} bytes ({:.1}x smaller)",
        source.len(),
        jpeg.len(),
        source.len() as f64 / jpeg.len() as f64
    );
    assert_eq!(attachment.mime_type, "image/jpeg");
    assert_eq!(attachment.name, "screenshot.jpg");
    assert_eq!(attachment.base64, base64(jpeg));
    assert_eq!(jpeg_size(jpeg), (2048, 1330));
    // Real contact sheets shrink about 3x; this one's noise is JPEG's worst.
    assert!(jpeg.len() < source.len());
    // A small one keeps its size.
    let small = super::image::attachment(Kind::Annotation, &screenshot(800, 500)).unwrap();
    assert_eq!(jpeg_size(&small.raw), (800, 500));
    assert_eq!(small.name, "annotation.jpg");
    // A transparent capture (a window's shadow) still encodes.
    let mut rgba = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut rgba, 64, 32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let pixels: Vec<u8> = (0..64 * 32)
            .flat_map(|i| [10, 20, 30, (i % 256) as u8])
            .collect();
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
    }
    assert_eq!(
        jpeg_size(
            &super::image::attachment(Kind::Screenshot, &rgba)
                .unwrap()
                .raw
        ),
        (64, 32)
    );
    // Not an image: no upload.
    assert_eq!(
        super::image::attachment(Kind::Screenshot, b"nope").err(),
        Some(Error::Image)
    );

    let thumb = super::image::thumbnail(jpeg).unwrap();
    eprintln!("its thumbnail: {} bytes", thumb.len());
    // Its short edge fills the 64-point square at 2x.
    assert_eq!(jpeg_size(&thumb), (198, 128));
    assert!(thumb.len() < 16 * 1024);
    assert_eq!(super::image::size(jpeg), Some((2048, 1330)));
    // A panorama's long edge stops at 512; a tiny image keeps its size.
    let wide = super::image::thumbnail(&screenshot(4000, 400)).unwrap();
    assert_eq!(jpeg_size(&wide), (512, 51));
    let tiny = super::image::thumbnail(&png(40, 30, |_, _| [9, 9, 9])).unwrap();
    assert_eq!(jpeg_size(&tiny), (40, 30));
}

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_disk_cache_is_private_and_lets_the_least_recently_used_go() {
    assert_eq!(super::cache::CAP, 50 * 1024 * 1024);
    let dir = TempDir::new();
    let folder = dir.0.join("slippy.cache");
    let full = super::image::attachment(Kind::Screenshot, &screenshot(1200, 800))
        .unwrap()
        .raw;
    let cache = DiskCache::new(folder.clone());
    assert!(cache.full(TS, "F1").is_none());
    let thumb = cache.store(TS, "F1", &full);
    assert_eq!(jpeg_size(&thumb), (192, 128));
    let file = |id: &str, kind: &str| folder.join(format!("{}-{id}.{kind}", TS.replace('.', "_")));
    assert_eq!(mode(&folder), 0o700);
    assert_eq!(mode(&file("F1", "img")), 0o600);
    assert_eq!(mode(&file("F1", "thumb")), 0o600);
    assert_eq!(cache.full(TS, "F1").as_deref(), Some(full.as_slice()));
    assert_eq!(cache.thumb(TS, "F1"), Some(thumb.clone()));
    // A lost thumbnail is made again from the full image.
    std::fs::remove_file(file("F1", "thumb")).unwrap();
    assert_eq!(jpeg_size(&cache.thumb(TS, "F1").unwrap()), (192, 128));
    assert!(file("F1", "thumb").exists());
    // Nothing outside the folder's names is read or written.
    assert!(cache.full("../x", "F1").is_none());
    assert!(cache.full(TS, "../F1").is_none());
    cache.store(TS, "../F9", &full);
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);
    // An upload waits for its id.
    cache.stage(TS, 0, &full);
    assert!(cache.thumb(TS, "U1").is_none());
    assert!(cache.claim(TS, 0, "U1"));
    assert!(!cache.claim(TS, 0, "U1"));
    assert_eq!(cache.full(TS, "U1").as_deref(), Some(full.as_slice()));
    std::fs::remove_dir_all(&folder).unwrap();

    // Room for three and a half: the fourth lets the least recently used go.
    let pair = (full.len() + thumb.len()) as u64;
    let cache = DiskCache::with_cap(folder.clone(), pair * 7 / 2);
    for id in ["F1", "F2", "F3"] {
        cache.store(TS, id, &full);
        thread::sleep(Duration::from_millis(20));
    }
    assert!(cache.thumb(TS, "F1").is_some(), "F1 used again");
    thread::sleep(Duration::from_millis(20));
    cache.store(TS, "F4", &full);
    let kept = |id: &str| file(id, "img").exists() && file(id, "thumb").exists();
    assert!(!file("F2", "img").exists() && !file("F2", "thumb").exists());
    assert!(kept("F1") && kept("F3") && kept("F4"));
    cache.clear();
    assert!(!folder.exists());
}

/// Waits for every command sent before it to finish.
fn barrier(handle: &sync::Handle) {
    let _ = handle.submit(|| Err(Error::Text)).unwrap().recv();
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

#[test]
fn an_upload_seeds_the_cache_so_its_thread_never_downloads_it() {
    let dir = TempDir::new();
    let ts = "991795815727.000001";
    let threads = Arc::new(Mutex::new(Threads {
        created: Some(ts.into()),
        ..Threads::default()
    }));
    let server = threads_server(threads.clone());
    let auth = server.client(&dir.0);
    let folder = auth.cache_dir(MEMBER);
    let worker = Worker::start(auth, MEMBER, true, Arc::new(|| {})).unwrap();
    let sent = worker
        .handle
        .image(
            Origin::Screenshot,
            screenshot(3024, 1964),
            "Fix this".into(),
        )
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(sent.ts, ts);
    let posted = server
        .requests()
        .into_iter()
        .find(|request| request.method == "POST")
        .unwrap()
        .json();
    assert_eq!(posted["files"][0]["mimeType"], "image/jpeg");
    assert_eq!(posted["files"][0]["name"], "screenshot.jpg");

    let key = (ts.to_owned(), "U1".to_owned());
    worker.handle.select(ts.into());
    wait_until("the thumbnail", || {
        worker
            .handle
            .snapshot()
            .thumbs
            .get(&key)
            .is_some_and(Option::is_some)
    });
    let thumb = worker.handle.snapshot().thumbs[&key].clone().unwrap();
    assert_eq!(jpeg_size(&thumb), (198, 128));
    worker.handle.open_preview(key.clone());
    let (open, full) = worker.handle.snapshot().preview_image.unwrap();
    assert_eq!((open, jpeg_size(&full)), (key.clone(), (2048, 1330)));
    worker.handle.close_preview();
    assert!(worker.handle.snapshot().preview_image.is_none());
    let downloads = || {
        server
            .requests()
            .iter()
            .filter(|request| request.path.contains("/files/"))
            .count()
    };
    barrier(&worker.handle);
    assert_eq!(downloads(), 0);

    // Hex quits (the folder stays) and starts again: selecting it populates
    // both its thread detail and thumbnail straight from disk, with zero
    // network calls for detail or files.
    let kept = dir.0.join("kept");
    copy_dir(&folder, &kept);
    worker.stop_and_join();
    assert!(!folder.exists(), "stopping (sign-out) deletes the folder");
    copy_dir(&kept, &folder);
    let details_before = server
        .requests()
        .iter()
        .filter(|request| request.path == format!("/api/hex/tasks/{ts}"))
        .count();
    let worker = Worker::start(server.client(&dir.0), MEMBER, true, Arc::new(|| {})).unwrap();
    wait_until("the reload", || worker.handle.snapshot().tasks.len() == 1);
    worker.handle.select(ts.into());
    let immediate = worker.handle.snapshot();
    assert!(
        matches!(immediate.detail, Some(Ok(ref detail)) if detail.task.ts == ts),
        "cached detail should appear synchronously on select"
    );
    assert_eq!(immediate.thumbs.get(&key), Some(&Some(thumb)));
    worker.handle.open_preview(key.clone());
    assert!(worker.handle.snapshot().preview_image.is_some());
    barrier(&worker.handle);
    assert_eq!(downloads(), 0);
    let details_after = server
        .requests()
        .iter()
        .filter(|request| request.path == format!("/api/hex/tasks/{ts}"))
        .count();
    assert_eq!(details_after, details_before);
    worker.stop_and_join();
    assert!(!folder.exists());
}

#[test]
fn a_thread_selected_again_shows_at_once_and_asks_again_only_once_it_moved() {
    let dir = TempDir::new();
    let (a, b) = ("991795815728.000001", "991795815728.000002");
    let threads = Arc::new(Mutex::new(Threads {
        tasks: vec![task(a, "done", 1), task(b, "done", 2)],
        messages: HashMap::from([
            (
                a.to_owned(),
                vec![answer("991795815728.000010", "A's answer")],
            ),
            (
                b.to_owned(),
                vec![answer("991795815728.000011", "B's answer")],
            ),
        ]),
        ..Threads::default()
    }));
    let server = threads_server(threads.clone());
    let worker = Worker::start(server.client(&dir.0), MEMBER, true, Arc::new(|| {})).unwrap();
    wait_until("the first load", || {
        worker.handle.snapshot().tasks.len() == 2
    });
    let asked = |ts: &str| {
        server
            .requests()
            .iter()
            .filter(|request| request.path == format!("/api/hex/tasks/{ts}"))
            .count()
    };
    let shown = || match worker.handle.snapshot().detail {
        Some(Ok(detail)) => Some((detail.task.ts, detail.task.seq)),
        _ => None,
    };
    worker.handle.select(a.into());
    barrier(&worker.handle);
    worker.handle.select(b.into());
    barrier(&worker.handle);
    assert_eq!((asked(a), asked(b)), (1, 1));
    assert_eq!(shown(), Some((b.to_owned(), 2)));

    // Back to A: its thread at once, no "Loading…", and no request.
    worker.handle.select(a.into());
    assert_eq!(shown(), Some((a.to_owned(), 1)));
    barrier(&worker.handle);
    worker.handle.poll_now();
    barrier(&worker.handle);
    assert_eq!(asked(a), 1);

    // A moves on: the next poll asks again.
    threads.lock().unwrap().tasks[0] = task(a, "running", 3);
    worker.handle.poll_now();
    barrier(&worker.handle);
    assert_eq!(asked(a), 2);
    assert_eq!(shown(), Some((a.to_owned(), 3)));
    // Refresh always asks.
    worker.handle.refresh();
    barrier(&worker.handle);
    assert_eq!(asked(a), 3);
    worker.stop_and_join();
}
