use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[test]
fn pkce_uses_unpadded_base64url_and_the_rfc_7636_s256_vector() {
    let pkce = loopback::Pkce {
        state: String::new(),
        verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
    };
    assert_eq!(
        pkce.challenge(),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    assert_eq!(loopback::base64url(&[]), "");
    assert_eq!(loopback::base64url(&[255]), "_w");
    assert_eq!(loopback::base64url(&[251, 255]), "-_8");
    assert_eq!(loopback::base64url(&[251, 255, 255]), "-___");
    let random = loopback::Pkce::new().unwrap();
    assert_eq!(random.state.len(), 43);
    assert_eq!(random.verifier.len(), 43);
    assert!(random.state != random.verifier);
}

#[test]
fn session_paths_are_outside_settings_and_config_backups() {
    let data = Path::new("/Users/test/Library/Application Support");
    for (test, name) in [(false, "hex-auth"), (true, "hex-auth-test")] {
        let path = session::session_path(None, Some(data), test).unwrap();
        assert_eq!(path, data.join(name).join("session.json"));
        assert!(!path.starts_with(data.join("hex")));
        assert!(!path.starts_with(data.join("hex-test")));
        assert!(!path.starts_with("/Users/test/.config/hex"));
    }
    let isolated = Path::new("/tmp/hex-fixture");
    assert_eq!(
        session::session_path(Some(isolated.as_os_str()), Some(data), false).unwrap(),
        isolated.join("hex-auth-session.json")
    );
}

#[test]
fn session_is_private_atomically_replaced_and_redacted() {
    let fixture = TempDir::new();
    let path = fixture.0.join("auth/session.json");
    let mut stored = StoredSession {
        origin: "https://example.com/".into(),
        client_id: "hex".into(),
        token_endpoint: "https://example.com/token".into(),
        access_token: "secret_access_value".into(),
        refresh_token: Some("secret_refresh_value".into()),
        access_expires_at_ms: None,
        member_sub: "principal".into(),
        email: "nick@dwa1.com".into(),
        redirect_port: Some(1337),
    };
    let debug = format!("{stored:?}");
    assert!(!debug.contains("secret_access_value"));
    assert!(!debug.contains("secret_refresh_value"));
    session::save(&path, &stored).unwrap();
    let previous = fs::File::open(&path).unwrap();
    stored.access_token = "rotated".into();
    session::save(&path, &stored).unwrap();
    let old: StoredSession = serde_json::from_reader(previous).unwrap();
    assert!(old.access_token == "secret_access_value");
    let next = session::load(&path).unwrap().unwrap();
    assert!(next.access_token == "rotated");
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    session::clear_session(&path).unwrap();
    session::clear_session(&path).unwrap();
    assert!(!path.exists());
}

#[test]
fn only_loopback_accepts_http_and_origins_cannot_carry_credentials() {
    for value in [
        "http://example.org",
        "http://127.0.0.1.evil.example",
        "https://user:secret@example.org",
        "https://example.org/path",
        "https://example.org?key=value",
    ] {
        assert_eq!(oauth::origin(value), Err(AuthError::Configuration));
    }
    assert!(oauth::origin("http://127.0.0.1:1234").is_ok());
    assert!(oauth::origin("https://example.org").is_ok());
}

#[test]
fn managed_oauth_sign_in_refresh_reauth_and_sign_out() {
    let fixture = TempDir::new();
    let server = Mock::start();
    let client = Client::new(fixture.0.join("session.json"), &server.origin, None).unwrap();
    let cancel = AtomicBool::new(false);
    let (open, page) = browser();
    let signed_in = client.sign_in(&cancel, open).unwrap();
    assert_member(signed_in);
    let page = page.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(page.0, 200);
    assert!(page.1.contains("Signed in to Hex. You can close this tab."));
    let stored = client.load().unwrap().unwrap();
    assert_eq!(stored.client_id, "registered-hex");
    assert_eq!(stored.member_sub, "member-id");
    assert!(stored.access_token == "first-access");

    // Verify the persisted session's identity even when the token isn't due.
    assert_member(client.refresh_if_needed(&cancel).unwrap());

    // A fresh-looking access token rejected by whoami also refreshes.
    server.state.lock().unwrap().reject_access = true;
    assert_member(client.refresh_if_needed(&cancel).unwrap());
    let stored = client.load().unwrap().unwrap();
    assert!(stored.access_token == "access-1");
    assert!(stored.refresh_token.as_deref() == Some("refresh-1"));

    // A Cloudflare Access login redirect on an expired bearer also refreshes.
    server.state.lock().unwrap().redirect_access_login = true;
    assert_member(client.refresh_if_needed(&cancel).unwrap());
    let mut stored = client.load().unwrap().unwrap();
    assert!(stored.access_token == "access-2");
    assert!(stored.refresh_token.as_deref() == Some("refresh-2"));

    // Refresh early without first sending a bearer that is about to expire.
    stored.access_expires_at_ms = Some(now_ms() + 20_000);
    session::save(&client.store, &stored).unwrap();
    let verification_count = server.state.lock().unwrap().verifications;
    assert_member(client.refresh_if_needed(&cancel).unwrap());
    assert_eq!(
        server.state.lock().unwrap().verifications,
        verification_count + 1
    );

    let mut stored = client.load().unwrap().unwrap();
    stored.access_expires_at_ms = Some(0);
    session::save(&client.store, &stored).unwrap();
    server.state.lock().unwrap().revoke_refresh = true;
    assert_eq!(
        client.refresh_if_needed(&cancel).unwrap(),
        AuthState::NeedsReauth {
            last_email: Some("nick@dwa1.com".into()),
            reason: AuthError::Refresh.message().into(),
        }
    );
    assert_eq!(client.sign_out().unwrap(), AuthState::SignedOut);
    assert!(!client.store.exists());
    assert_eq!(
        client.refresh_if_needed(&cancel).unwrap(),
        AuthState::SignedOut
    );
}

#[test]
fn callback_state_mismatch_never_exchanges_a_code_or_saves_a_session() {
    let fixture = TempDir::new();
    let server = Mock::start();
    server.state.lock().unwrap().wrong_state = true;
    let client = Client::new(fixture.0.join("session.json"), &server.origin, None).unwrap();
    let (open, page) = browser();
    assert_eq!(
        client.sign_in(&AtomicBool::new(false), open),
        Err(AuthError::Callback)
    );
    assert_eq!(page.recv_timeout(Duration::from_secs(5)).unwrap().0, 400);
    let state = server.state.lock().unwrap();
    assert_eq!(state.verifications, 0);
    assert_eq!(state.code_exchanges, 0);
    assert!(!client.store.exists());
}

#[test]
fn client_override_uses_well_known_discovery_without_registration() {
    let fixture = TempDir::new();
    let server = Mock::start();
    {
        let mut state = server.state.lock().unwrap();
        state.challenge = false;
        state.registered_id = Some("static-hex".into());
    }
    let client = Client::new(
        fixture.0.join("session.json"),
        &server.origin,
        Some("static-hex".into()),
    )
    .unwrap();
    let (open, page) = browser();
    assert_member(client.sign_in(&AtomicBool::new(false), open).unwrap());
    assert_eq!(page.recv_timeout(Duration::from_secs(5)).unwrap().0, 200);
    let state = server.state.lock().unwrap();
    assert_eq!(state.registrations, 0);
    assert_eq!(state.resource_discoveries, 1);
}

#[test]
fn cancelling_the_browser_wait_never_writes_a_session() {
    let fixture = TempDir::new();
    let server = Mock::start();
    let client = Client::new(fixture.0.join("session.json"), &server.origin, None).unwrap();
    let cancel = AtomicBool::new(false);
    let open = |_: &Url| {
        cancel.store(true, Ordering::Release);
        Ok(())
    };
    assert_eq!(client.sign_in(&cancel, open), Err(AuthError::Cancelled));
    assert!(!client.store.exists());
}

/// Signs in against `server`, then leaves a stale access token behind, as
/// if another Hex process or an expired token got there first.
fn signed_in_with_stale_token(server: &Mock, fixture: &TempDir) -> Client {
    let client = Client::new(fixture.0.join("session.json"), &server.origin, None).unwrap();
    let (open, page) = browser();
    assert_member(client.sign_in(&AtomicBool::new(false), open).unwrap());
    page.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut stored = client.load().unwrap().unwrap();
    stored.access_token = "stale-access".into();
    session::save(&client.store, &stored).unwrap();
    client
}

#[test]
fn concurrent_rejections_share_one_refresh() {
    let fixture = TempDir::new();
    let server = Mock::start();
    let client = Arc::new(signed_in_with_stale_token(&server, &fixture));
    let cancel = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let (client, cancel, barrier) = (client.clone(), cancel.clone(), barrier.clone());
            thread::spawn(move || {
                barrier.wait();
                client
                    .send_authorized(
                        "member-id",
                        "/api/hex/tasks?since=0&limit=50",
                        None,
                        &cancel,
                    )
                    .map(|response| response.status().as_u16())
            })
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), Ok(200));
    }
    let state = server.state.lock().unwrap();
    // A second refresh would have sent the rotated-away refresh token,
    // which the mock refuses with a panic.
    assert_eq!(state.refreshes, 1);
    assert!(
        state
            .tasks
            .iter()
            .all(|(token, _)| token == "Bearer stale-access" || token == "Bearer access-1")
    );
    drop(state);
    assert!(client.load().unwrap().unwrap().access_token == "access-1");
}

#[test]
fn an_access_login_redirect_refreshes_and_resends_the_same_body() {
    let fixture = TempDir::new();
    let server = Mock::start();
    let client = Client::new(fixture.0.join("session.json"), &server.origin, None).unwrap();
    let (open, page) = browser();
    assert_member(client.sign_in(&AtomicBool::new(false), open).unwrap());
    page.recv_timeout(Duration::from_secs(5)).unwrap();
    server.state.lock().unwrap().redirect_access_login = true;
    let body = br#"{"idempotencyKey":"same-key","origin":"dictation","text":"hi","files":[]}"#;
    let response = client
        .send_authorized(
            "member-id",
            "/api/hex/tasks",
            Some(body),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let state = server.state.lock().unwrap();
    assert_eq!(state.refreshes, 1);
    assert_eq!(state.tasks.len(), 2);
    assert_eq!(state.tasks[0].0, "Bearer first-access");
    assert_eq!(state.tasks[1].0, "Bearer access-1");
    assert!(state.tasks.iter().all(|(_, sent)| sent == body));
}

#[test]
fn a_bearer_still_rejected_after_refresh_needs_sign_in() {
    let fixture = TempDir::new();
    let server = Mock::start();
    let client = signed_in_with_stale_token(&server, &fixture);
    server.state.lock().unwrap().reject_tasks = true;
    let cancel = AtomicBool::new(false);
    assert_eq!(
        client
            .send_authorized("member-id", "/api/hex/tasks", None, &cancel)
            .err(),
        Some(AuthError::Refresh)
    );
    // Another account's session, or another origin, is never sent.
    assert_eq!(
        client
            .send_authorized("someone-else", "/api/hex/tasks", None, &cancel)
            .err(),
        Some(AuthError::Refresh)
    );
    assert_eq!(
        client
            .send_authorized("member-id", "https://evil.example/api", None, &cancel)
            .err(),
        Some(AuthError::Configuration)
    );
    cancel.store(true, Ordering::Release);
    assert_eq!(
        client
            .send_authorized("member-id", "/api/hex/tasks", None, &cancel)
            .err(),
        Some(AuthError::Cancelled)
    );
    assert_eq!(server.state.lock().unwrap().tasks.len(), 2);
}

fn assert_member(state: AuthState) {
    assert!(matches!(
        state,
        AuthState::SignedIn { email, member_sub, expires_at_ms: Some(_) }
            if email == "nick@dwa1.com" && member_sub == "member-id"
    ));
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let mut bytes = [0; 12];
        getrandom::fill(&mut bytes).unwrap();
        let path =
            std::env::temp_dir().join(format!("hex-team-auth-{}", loopback::base64url(&bytes)));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

type BrowserPage = std::sync::mpsc::Receiver<(u16, String)>;

fn browser() -> (impl FnOnce(&Url) -> io::Result<()>, BrowserPage) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let open = move |url: &Url| {
        let url = url.clone();
        thread::spawn(move || {
            let browser = crate::net::loopback_agent(|builder| {
                builder
                    .max_redirects(0)
                    .http_status_as_error(false)
                    .timeout_global(Some(Duration::from_secs(5)))
            });
            let response = browser.get(url.as_str()).call().unwrap();
            assert_eq!(response.status().as_u16(), 302);
            let location = response
                .headers()
                .get("Location")
                .unwrap()
                .to_str()
                .unwrap();
            let mut callback = browser.get(location).call().unwrap();
            let status = callback.status().as_u16();
            let html = callback.body_mut().read_to_string().unwrap();
            sender.send((status, html)).unwrap();
        });
        Ok(())
    };
    (open, receiver)
}

struct Mock {
    origin: String,
    state: Arc<Mutex<MockState>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct MockState {
    origin: String,
    challenge: bool,
    registered_id: Option<String>,
    redirect: Option<String>,
    pkce_challenge: Option<String>,
    reject_access: bool,
    redirect_access_login: bool,
    revoke_refresh: bool,
    wrong_state: bool,
    registrations: usize,
    resource_discoveries: usize,
    code_exchanges: usize,
    refreshes: usize,
    verifications: usize,
    /// `/api/hex/tasks` rejects every bearer, even a fresh one.
    reject_tasks: bool,
    /// Each `/api/hex/tasks` request's bearer and body.
    tasks: Vec<(String, Vec<u8>)>,
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://{address}");
        let state = Arc::new(Mutex::new(MockState {
            origin: origin.clone(),
            challenge: true,
            registered_id: None,
            redirect: None,
            pkce_challenge: None,
            reject_access: false,
            redirect_access_login: false,
            revoke_refresh: false,
            wrong_state: false,
            registrations: 0,
            resource_discoveries: 0,
            code_exchanges: 0,
            refreshes: 0,
            verifications: 0,
            reject_tasks: false,
            tasks: Vec::new(),
        }));
        let server_state = Arc::clone(&state);
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if server_stop.load(Ordering::Acquire) {
                    break;
                }
                let mut stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                server_state.lock().unwrap().request(&mut stream);
            }
        });
        Self {
            origin,
            state,
            stop,
            thread: Some(thread),
        }
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

impl MockState {
    fn resource(&self) -> String {
        if self.challenge {
            format!("{}/api/hex/whoami", self.origin)
        } else {
            self.origin.clone()
        }
    }

    fn request(&mut self, stream: &mut TcpStream) {
        let mut reader = BufReader::new(&mut *stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut parts = line.split_ascii_whitespace();
        let method = parts.next().unwrap().to_owned();
        let url = Url::parse(&format!("{}{}", self.origin, parts.next().unwrap())).unwrap();
        let mut headers = HashMap::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
            let (name, value) = line.split_once(':').unwrap();
            headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
        let size = headers
            .get("content-length")
            .map_or(0, |length| length.parse().unwrap());
        let mut data = vec![0; size];
        reader.read_exact(&mut data).unwrap();

        use serde_json::json;
        match (method.as_str(), url.path()) {
            ("GET", "/api/hex/whoami") => {
                let Some(token) = headers.get("authorization") else {
                    let challenge = format!(
                        "Bearer realm=\"OAuth\", error=\"invalid_token\", error_description=\"Missing or invalid access token\", resource_metadata=\"{}/.well-known/cloudflare-access-protected-resource/api/hex/whoami\"",
                        self.origin
                    );
                    let headers: &[_] = if self.challenge {
                        &[("WWW-Authenticate", challenge.as_str())]
                    } else {
                        &[]
                    };
                    return respond(stream, "401 Unauthorized", headers, &json!({}));
                };
                self.verifications += 1;
                let expected = if self.refreshes == 0 {
                    "Bearer first-access".into()
                } else {
                    format!("Bearer access-{}", self.refreshes)
                };
                if self.redirect_access_login {
                    return respond(
                        stream,
                        "302 Found",
                        &[(
                            "Location",
                            "https://slipstream-advisors.cloudflareaccess.com/cdn-cgi/access/login/hex.theslipstreamadvisors.com",
                        )],
                        &json!({}),
                    );
                }
                if self.reject_access || token != &expected {
                    return respond(stream, "401 Unauthorized", &[], &json!({}));
                }
                respond(
                    stream,
                    "200 OK",
                    &[],
                    &json!({"principal": "member", "sub": "member-id", "email": "nick@dwa1.com"}),
                );
            }
            (
                "GET",
                path @ ("/.well-known/cloudflare-access-protected-resource/api/hex/whoami"
                | "/.well-known/oauth-protected-resource"),
            ) => {
                assert_eq!(
                    path != "/.well-known/oauth-protected-resource",
                    self.challenge
                );
                self.resource_discoveries += 1;
                let issuer = if self.challenge {
                    self.origin.clone()
                } else {
                    format!("{}/issuer", self.origin)
                };
                respond(
                    stream,
                    "200 OK",
                    &[],
                    &json!({"resource": self.resource(), "authorization_servers": [issuer]}),
                );
            }
            (
                "GET",
                path @ ("/.well-known/oauth-authorization-server"
                | "/.well-known/oauth-authorization-server/issuer"),
            ) => {
                assert_eq!(
                    path == "/.well-known/oauth-authorization-server",
                    self.challenge
                );
                assert_eq!(self.resource_discoveries, 1);
                respond(
                    stream,
                    "200 OK",
                    &[],
                    &json!({
                        "authorization_endpoint": format!("{}/cdn-cgi/access/oauth/authorization", self.origin),
                        "token_endpoint": format!("{}/cdn-cgi/access/oauth/token", self.origin),
                        "registration_endpoint": format!("{}/cdn-cgi/access/oauth/registration", self.origin),
                        "grant_types_supported": ["authorization_code", "refresh_token"],
                        "code_challenge_methods_supported": ["S256"],
                        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post", "none"],
                        "response_types_supported": ["code"],
                    }),
                );
            }
            ("POST", "/cdn-cgi/access/oauth/registration") => {
                self.registrations += 1;
                assert_eq!(headers["content-type"], "application/json");
                let reg: serde_json::Value = serde_json::from_slice(&data).unwrap();
                assert_eq!(reg["client_name"], "Hex");
                assert_eq!(
                    reg["grant_types"],
                    json!(["authorization_code", "refresh_token"])
                );
                assert_eq!(reg["response_types"], json!(["code"]));
                assert_eq!(reg["token_endpoint_auth_method"], "none");
                self.redirect = Some(reg["redirect_uris"][0].as_str().unwrap().to_owned());
                self.registered_id = Some("registered-hex".into());
                respond(
                    stream,
                    "201 Created",
                    &[],
                    &json!({"client_id": "registered-hex", "redirect_uris": [self.redirect]}),
                );
            }
            ("GET", "/cdn-cgi/access/oauth/authorization") => {
                let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
                assert_eq!(query["response_type"], "code");
                assert_eq!(Url::parse(&query["resource"]), Url::parse(&self.resource()));
                assert!(!query.contains_key("scope"));
                assert_eq!(query["code_challenge_method"], "S256");
                assert_eq!(Some(&query["client_id"]), self.registered_id.as_ref());
                let mut callback = Url::parse(&query["redirect_uri"]).unwrap();
                assert_eq!(callback.host_str(), Some("127.0.0.1"));
                assert_eq!(callback.path(), "/callback");
                if let Some(redirect) = &self.redirect {
                    assert_eq!(callback.as_str(), redirect);
                }
                self.redirect = Some(callback.to_string());
                self.pkce_challenge = Some(query["code_challenge"].clone());
                let state = if self.wrong_state {
                    "wrong-state"
                } else {
                    &query["state"]
                };
                callback
                    .query_pairs_mut()
                    .append_pair("code", "test-code")
                    .append_pair("state", state);
                respond(
                    stream,
                    "302 Found",
                    &[("Location", callback.as_str())],
                    &json!({}),
                );
            }
            ("POST", "/cdn-cgi/access/oauth/token") => {
                assert_eq!(headers["content-type"], "application/x-www-form-urlencoded");
                let form: HashMap<_, _> = url::form_urlencoded::parse(&data).into_owned().collect();
                assert_eq!(Some(&form["client_id"]), self.registered_id.as_ref());
                if form["grant_type"] == "authorization_code" {
                    self.code_exchanges += 1;
                    assert_eq!(form["code"], "test-code");
                    assert_eq!(Url::parse(&form["resource"]), Url::parse(&self.resource()));
                    assert_eq!(Some(&form["redirect_uri"]), self.redirect.as_ref());
                    let digest = Sha256::digest(form["code_verifier"].as_bytes());
                    assert_eq!(Some(loopback::base64url(&digest)), self.pkce_challenge);
                    respond(
                        stream,
                        "200 OK",
                        &[],
                        &json!({"access_token": "first-access", "refresh_token": "first-refresh", "expires_in": 3600}),
                    );
                } else {
                    assert_eq!(form["grant_type"], "refresh_token");
                    let expected = if self.refreshes == 0 {
                        "first-refresh".into()
                    } else {
                        format!("refresh-{}", self.refreshes)
                    };
                    assert!(form["refresh_token"] == expected);
                    if self.revoke_refresh {
                        return respond(
                            stream,
                            "400 Bad Request",
                            &[],
                            &json!({"error": "invalid_grant"}),
                        );
                    }
                    self.refreshes += 1;
                    self.reject_access = false;
                    self.redirect_access_login = false;
                    respond(
                        stream,
                        "200 OK",
                        &[],
                        &json!({
                            "access_token": format!("access-{}", self.refreshes),
                            "refresh_token": format!("refresh-{}", self.refreshes),
                            "expires_in": 3600,
                        }),
                    );
                }
            }
            (_, "/api/hex/tasks") => {
                let token = headers.get("authorization").cloned().unwrap_or_default();
                self.tasks.push((token.clone(), data));
                let expected = if self.refreshes == 0 {
                    "Bearer first-access".into()
                } else {
                    format!("Bearer access-{}", self.refreshes)
                };
                if self.redirect_access_login {
                    return respond(
                        stream,
                        "302 Found",
                        &[(
                            "Location",
                            "https://slipstream-advisors.cloudflareaccess.com/cdn-cgi/access/login/hex.theslipstreamadvisors.com",
                        )],
                        &json!({}),
                    );
                }
                if self.reject_tasks || token != expected {
                    return respond(stream, "401 Unauthorized", &[], &json!({}));
                }
                respond(stream, "200 OK", &[], &json!({"cursor": 0, "tasks": []}));
            }
            _ => panic!("unexpected OAuth route"),
        }
    }
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(&str, &str)],
    value: &serde_json::Value,
) {
    let data = value.to_string();
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", data.len()).unwrap();
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n").unwrap();
    }
    write!(stream, "\r\n{data}").unwrap();
}

/// Sequential authorized requests reuse one keep-alive connection instead
/// of a new TCP (and, at the real origin, TLS) handshake each.
#[test]
fn authorized_requests_share_one_kept_alive_connection() {
    use std::sync::atomic::AtomicUsize;
    let fixture = TempDir::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (accepted, stopped) = (connections.clone(), stop.clone());
    let server = thread::spawn(move || {
        for stream in listener.incoming() {
            if stopped.load(Ordering::Acquire) {
                break;
            }
            let Ok(mut stream) = stream else { continue };
            accepted.fetch_add(1, Ordering::AcqRel);
            thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    // One request: its line and headers (GETs have no body).
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
                    );
                }
            });
        }
    });
    let client = test_client(
        fixture.0.join("session.json"),
        &format!("http://{address}"),
        "member-sub",
        "access-0",
    );
    let cancelled = AtomicBool::new(false);
    for path in [
        "/api/hex/tasks",
        "/api/hex/tasks/1",
        "/api/hex/tasks/1/files/F1",
    ] {
        let mut response = client
            .send_authorized("member-sub", path, None, &cancelled)
            .unwrap();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.body_mut().read_to_vec().unwrap(), b"{}");
    }
    assert_eq!(connections.load(Ordering::Acquire), 1);
    drop(client);
    stop.store(true, Ordering::Release);
    let _ = TcpStream::connect(address);
    server.join().unwrap();
}
