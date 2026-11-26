//! The macOS Dev lab's Cloudflare Access session. UI callers run these
//! blocking operations on a single worker and cancel it when the lab turns off.

mod loopback;
mod oauth;
mod session;
#[cfg(test)]
mod tests;

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use session::StoredSession;
use url::Url;

const DEFAULT_ORIGIN: &str = "https://hex.theslipstreamadvisors.com";
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

    fn new(store: PathBuf, origin: &str, client_id: Option<String>) -> Result<Self, AuthError> {
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
            |session| self.save(session, cancelled),
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
        let due = session
            .access_expires_at_ms
            .is_some_and(|expires| expires <= now_ms().saturating_add(REFRESH_WINDOW_MS));
        let member = if due {
            None
        } else {
            check_cancelled(cancelled)?;
            oauth::whoami(&self.origin, &session.access_token)?
        };
        let member = match member {
            Some(member) => member,
            None => {
                check_cancelled(cancelled)?;
                oauth::refresh(&self.origin, session)?;
                // Save rotated credentials before another request can fail.
                self.save(session, cancelled)?;
                check_cancelled(cancelled)?;
                oauth::whoami(&self.origin, &session.access_token)?.ok_or(AuthError::Refresh)?
            }
        };
        if member.sub != session.member_sub {
            return Err(AuthError::Verification);
        }
        if member.email != session.email {
            session.email = member.email;
            self.save(session, cancelled)?;
        }
        check_cancelled(cancelled)
    }

    pub(crate) fn sign_out(&self) -> Result<AuthState, AuthError> {
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
