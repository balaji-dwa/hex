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

use super::client::{Attachment, Client, Error, Origin, Status, base64, valid_ts};
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

fn respond(stream: &mut TcpStream, status: u16, headers: &[(&str, String)], body: &Value) {
    let data = body.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: private, no-store\r\n",
        data.len()
    );
    for (name, value) in headers {
        let _ = write!(stream, "{name}: {value}\r\n");
    }
    let _ = write!(stream, "\r\n{data}");
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
    // Base64 of the upload stays under the server's 8 MiB decoded limit.
    let attachment = super::image::attachment("screenshot.png", &source).unwrap();
    assert!(attachment.base64.len() / 4 * 3 < 8 * 1024 * 1024);
    assert_eq!(attachment.mime_type, "image/png");
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
    assert_eq!(attachment.name, "annotation.png");
    assert_eq!(attachment.mime_type, "image/png");
    scrawl_capture::bundle::publish(&bundle, &manifest("  ")).unwrap();
    assert_eq!(super::bundle::read(&bundle).unwrap().0, "Annotation");
    std::fs::remove_file(bundle.join("sheet.png")).unwrap();
    assert_eq!(super::bundle::read(&bundle).err(), Some(Error::Image));
}
