//! The macOS Dev lab's Cloudflare Access session. UI callers run these
//! blocking operations on a single worker and cancel it when the lab turns off.

mod loopback;
mod oauth;
mod session;
#[cfg(test)]
mod tests;

use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use session::StoredSession;
use url::Url;

const DEFAULT_ORIGIN: &str = "https://hex.theslipstreamadvisors.com";
static REFRESH: Mutex<()> = Mutex::new(());
pub(crate) const REFRESH_WINDOW_MS: u64 = 60_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AuthState {
    SignedOut,
    SigningIn {
        started_at_ms: u64,
    },
    SignedIn {
        email: String,
        member_sub: String,
        expires_at_ms: Option<u64>,
    },
    NeedsReauth {
        last_email: Option<String>,
        reason: String,
    },
}

/// User-facing errors deliberately omit request URLs, server bodies and headers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AuthError {
    Cancelled,
    Configuration,
    Service,
    Callback,
    Browser,
    Timeout,
    Denied,
    Verification,
    Refresh,
    Storage,
}

impl AuthError {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "Sign-in canceled.",
            Self::Configuration => "Team sign-in is configured incorrectly.",
            Self::Service => "Couldn't reach team sign-in. Try again.",
            Self::Callback => "Couldn't verify the browser response. Try again.",
            Self::Browser => "Couldn't open your browser.",
            Self::Timeout => "Sign-in timed out. Try again.",
            Self::Denied => "Sign-in was declined.",
            Self::Verification => "Couldn't verify your team account. Try again.",
            Self::Refresh => "Your team session expired. Sign in again.",
            Self::Storage => "Couldn't access the saved team session.",
        }
    }
}

pub(crate) struct Client {
    store: PathBuf,
    origin: Url,
    client_id: Option<String>,
}

impl Client {
    pub(crate) fn configured() -> Result<Self, AuthError> {
        let store = session::path().map_err(|_| AuthError::Storage)?;
        let origin =
            std::env::var("HEX_TEAM_AUTH_ORIGIN").unwrap_or_else(|_| DEFAULT_ORIGIN.into());
        Self::new(
            store,
            &origin,
            std::env::var("HEX_TEAM_AUTH_CLIENT_ID").ok(),
        )
    }

    pub(crate) fn new(
        store: PathBuf,
        origin: &str,
        client_id: Option<String>,
    ) -> Result<Self, AuthError> {
        Ok(Self {
            store,
            origin: oauth::origin(origin)?,
            client_id: client_id.filter(|id| !id.is_empty()),
        })
    }

    pub(crate) fn sign_in(
        &self,
        cancelled: &AtomicBool,
        open: impl FnOnce(&Url) -> io::Result<()>,
    ) -> Result<AuthState, AuthError> {
        check_cancelled(cancelled)?;
        let cached = self.load()?;
        let session = oauth::sign_in(
            &self.origin,
            self.client_id.as_deref(),
            cached,
            cancelled,
            open,
            |session| {
                let _refresh = REFRESH.lock().map_err(|_| AuthError::Storage)?;
                self.save(session, cancelled)
            },
        )?;
        Ok(signed_in(session))
    }

    pub(crate) fn refresh_if_needed(&self, cancelled: &AtomicBool) -> Result<AuthState, AuthError> {
        check_cancelled(cancelled)?;
        let Some(mut session) = self.load()? else {
            return Ok(AuthState::SignedOut);
        };
        let last_email = Some(session.email.clone());
        Ok(match self.whoami(&mut session, cancelled) {
            Ok(()) => signed_in(session),
            Err(error) => AuthState::NeedsReauth {
                last_email,
                reason: error.message().into(),
            },
        })
    }

    fn whoami(&self, session: &mut StoredSession, cancelled: &AtomicBool) -> Result<(), AuthError> {
        let member = if refresh_due(session) {
            None
        } else {
            check_cancelled(cancelled)?;
            oauth::whoami(&self.origin, &session.access_token)?
        };
        let member = match member {
            Some(member) => member,
            None => {
                *session = self.refresh(session, cancelled)?;
                check_cancelled(cancelled)?;
                oauth::whoami(&self.origin, &session.access_token)?.ok_or(AuthError::Refresh)?
            }
        };
        if member.sub != session.member_sub {
            return Err(AuthError::Verification);
        }
        if member.email != session.email {
            let _refresh = REFRESH.lock().map_err(|_| AuthError::Storage)?;
            let mut current = self.load_member(&session.member_sub)?;
            current.email = member.email;
            self.save(&current, cancelled)?;
            *session = current;
        }
        check_cancelled(cancelled)
    }

    /// Send a JSON POST or a GET to the configured team origin. The same
    /// serialized body is reused if Access rejects the first token.
    pub(crate) fn send_authorized(
        &self,
        member_sub: &str,
        path: &str,
        body: Option<&[u8]>,
        cancelled: &AtomicBool,
    ) -> Result<ureq::http::Response<ureq::Body>, AuthError> {
        let url = self
            .origin
            .join(path)
            .map_err(|_| AuthError::Configuration)?;
        if !path.starts_with('/') || self.origin.origin() != url.origin() {
            return Err(AuthError::Configuration);
        }
        check_cancelled(cancelled)?;
        let mut session = self.load_member(member_sub)?;
        if refresh_due(&session) {
            session = self.refresh(&session, cancelled)?;
        }
        let agent = oauth::agent_with_timeout(
            &url,
            Duration::from_secs(if body.is_some() { 45 } else { 12 }),
        );
        let send = |session: &StoredSession| {
            check_cancelled(cancelled)?;
            let bearer = format!("Bearer {}", session.access_token);
            match body {
                Some(body) => agent
                    .post(url.as_str())
                    .header("Authorization", bearer)
                    .header("Content-Type", "application/json")
                    .send(body),
                None => agent
                    .get(url.as_str())
                    .header("Authorization", bearer)
                    .call(),
            }
            .map_err(|_| AuthError::Service)
        };
        let response = send(&session)?;
        if !oauth::access_rejected(&response) {
            check_cancelled(cancelled)?;
            return Ok(response);
        }
        drop(response);
        let response = send(&self.refresh(&session, cancelled)?)?;
        check_cancelled(cancelled)?;
        if oauth::access_rejected(&response) {
            return Err(AuthError::Refresh);
        }
        Ok(response)
    }

    pub(crate) fn cursor_path(&self, member_sub: &str) -> PathBuf {
        use sha2::{Digest, Sha256};
        use std::fmt::Write;
        let digest = Sha256::digest(format!("{}\0{member_sub}", self.origin));
        let mut name = String::from("slippy-");
        for byte in digest.iter() {
            let _ = write!(name, "{byte:02x}");
        }
        self.store.with_extension(format!("{name}.json"))
    }

    fn load_member(&self, member_sub: &str) -> Result<StoredSession, AuthError> {
        self.load()?
            .filter(|session| session.member_sub == member_sub)
            .ok_or(AuthError::Refresh)
    }

    fn refresh(
        &self,
        observed: &StoredSession,
        cancelled: &AtomicBool,
    ) -> Result<StoredSession, AuthError> {
        let _refresh = REFRESH.lock().map_err(|_| AuthError::Storage)?;
        check_cancelled(cancelled)?;
        let mut current = self.load_member(&observed.member_sub)?;
        if current.access_token == observed.access_token {
            oauth::refresh(&self.origin, &mut current)?;
            // Rotation already happened at the server, even if the lab was
            // disabled while the token request was in flight.
            session::save(&self.store, &current).map_err(|_| AuthError::Storage)?;
        }
        check_cancelled(cancelled)?;
        Ok(current)
    }

    pub(crate) fn sign_out(&self) -> Result<AuthState, AuthError> {
        let _refresh = REFRESH.lock().map_err(|_| AuthError::Storage)?;
        session::clear_session(&self.store).map_err(|_| AuthError::Storage)?;
        Ok(AuthState::SignedOut)
    }

    fn load(&self) -> Result<Option<StoredSession>, AuthError> {
        session::load(&self.store)
            .map(|stored| stored.filter(|stored| stored.origin == self.origin.as_str()))
            .map_err(|_| AuthError::Storage)
    }

    fn save(&self, session: &StoredSession, cancelled: &AtomicBool) -> Result<(), AuthError> {
        check_cancelled(cancelled)?;
        session::save(&self.store, session).map_err(|_| AuthError::Storage)
    }
}

/// A signed-in client for `member_sub` at a loopback `origin`, for the
/// Slippy tests' mock server.
#[cfg(test)]
pub(crate) fn test_client(store: PathBuf, origin: &str, member_sub: &str, access: &str) -> Client {
    let client = Client::new(store, origin, Some("hex-test".into())).unwrap();
    session::save(
        &client.store,
        &StoredSession {
            origin: client.origin.to_string(),
            client_id: "hex-test".into(),
            token_endpoint: format!("{}cdn-cgi/access/oauth/token", client.origin),
            access_token: access.into(),
            refresh_token: Some("refresh-0".into()),
            access_expires_at_ms: Some(now_ms() + 3_600_000),
            member_sub: member_sub.into(),
            email: "nick@dwa1.com".into(),
            redirect_port: None,
        },
    )
    .unwrap();
    client
}

fn refresh_due(session: &StoredSession) -> bool {
    session
        .access_expires_at_ms
        .is_some_and(|expires| expires <= now_ms().saturating_add(REFRESH_WINDOW_MS))
}

fn signed_in(session: StoredSession) -> AuthState {
    AuthState::SignedIn {
        email: session.email,
        member_sub: session.member_sub,
        expires_at_ms: session.access_expires_at_ms,
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), AuthError> {
    if cancelled.load(Ordering::Acquire) {
        Err(AuthError::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().try_into().unwrap_or(u64::MAX)
        })
}
