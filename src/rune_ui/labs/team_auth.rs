use std::io;
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};
use std::time::Duration;

use rune::{El, Tx, row, text};
use url::Url;

use super::super::{
    Ui,
    controls::{self, ButtonKind, Paint},
};
use crate::desktop::i18n::{tr, tr_fill};
use crate::team_auth::{AuthError, AuthState, Client, REFRESH_WINDOW_MS, now_ms};

pub(in crate::rune_ui) struct Card {
    pub(in crate::rune_ui) state: AuthState,
    checked: bool,
    next_refresh: Option<u64>,
    job: Option<Job>,
}

impl Default for Card {
    fn default() -> Self {
        Self {
            state: AuthState::SignedOut,
            checked: false,
            next_refresh: None,
            job: None,
        }
    }
}

struct Job {
    cancel: Arc<AtomicBool>,
    result: Receiver<Result<AuthState, AuthError>>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
enum Action {
    Check,
    SignIn,
    SignOut,
}

impl Action {
    fn run(self, cancel: &AtomicBool) -> Result<AuthState, AuthError> {
        if cancel.load(Ordering::Acquire) {
            return Err(AuthError::Cancelled);
        }
        let client = Client::configured()?;
        match self {
            Self::Check => client.refresh_if_needed(cancel),
            Self::SignIn => client.sign_in(cancel, open_browser),
            Self::SignOut => client.sign_out(),
        }
    }
}

impl Card {
    pub(in crate::rune_ui) fn disable(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Release);
        }
        self.state = AuthState::SignedOut;
        self.checked = false;
        self.next_refresh = None;
    }

    #[cfg(target_os = "macos")]
    pub(in crate::rune_ui) fn needs_reauth(&mut self, member: &str, error: AuthError) {
        if matches!(&self.state, AuthState::SignedIn { member_sub, .. } if member_sub == member) {
            self.state = reauth(&self.state, error);
            self.next_refresh = None;
            if let Some(job) = &self.job {
                job.cancel.store(true, Ordering::Release);
            }
        }
    }

    #[cfg(target_os = "macos")]
    pub(in crate::rune_ui) fn signed_out(&self) -> bool {
        self.checked && self.job.is_none() && !matches!(self.state, AuthState::SignedIn { .. })
    }

    fn start(&mut self, action: Action) -> bool {
        if self.job.is_some() {
            return false;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (sender, result) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("hex-team-auth".into())
            .spawn(move || {
                let _ = sender.send(action.run(&worker_cancel));
            });
        if started.is_err() {
            self.state = reauth(&self.state, AuthError::Service);
            return true;
        }
        self.job = Some(Job { cancel, result });
        match action {
            Action::SignIn => {
                self.state = AuthState::SigningIn {
                    started_at_ms: now_ms(),
                }
            }
            Action::SignOut => {
                self.state = AuthState::SignedOut;
                self.next_refresh = None;
                #[cfg(target_os = "macos")]
                crate::slippy::sign_out();
            }
            Action::Check => {}
        }
        true
    }

    /// A check starts at launch once the lab is on, whether or not the
    /// window is open, so Slippy's worker and notifications start too; then
    /// again when the access token is due.
    fn check_due(&self, enabled: bool) -> bool {
        enabled && (!self.checked || self.next_refresh.is_some_and(|at| now_ms() >= at))
    }

    fn poll(&mut self, enabled: bool, tx: &mut Tx) -> bool {
        let mut changed = false;
        if let Some(job) = &self.job {
            let result = match job.result.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Disconnected) => Err(AuthError::Service),
                Err(TryRecvError::Empty) => {
                    tx.wake_after(Duration::from_millis(100));
                    return false;
                }
            };
            if enabled && self.checked {
                self.state = match result {
                    Ok(state) => state,
                    Err(AuthError::Cancelled) => AuthState::SignedOut,
                    Err(error) => reauth(&self.state, error),
                };
                self.next_refresh = match self.state {
                    AuthState::SignedIn {
                        expires_at_ms: Some(expires),
                        ..
                    } => {
                        let early = expires.saturating_sub(REFRESH_WINDOW_MS);
                        Some(if early > now_ms() { early } else { expires })
                    }
                    _ => None,
                };
            }
            self.job = None;
            changed = true;
        }
        if self.check_due(enabled) {
            self.checked = true;
            self.next_refresh = None;
            changed |= self.start(Action::Check);
            tx.wake_after(Duration::from_millis(100));
        }
        if enabled && let Some(at) = self.next_refresh {
            tx.wake_after(Duration::from_millis(at.saturating_sub(now_ms())));
        }
        changed
    }
}

fn reauth(state: &AuthState, error: AuthError) -> AuthState {
    let last_email = match state {
        AuthState::SignedIn { email, .. } => Some(email.clone()),
        AuthState::NeedsReauth { last_email, .. } => last_email.clone(),
        _ => None,
    };
    AuthState::NeedsReauth {
        last_email,
        reason: error.message().into(),
    }
}

pub(in crate::rune_ui) fn poll(ui: &mut Ui, tx: &mut Tx) -> bool {
    let enabled = ui.live() && crate::labs::team_auth(&ui.settings);
    let changed = ui.lab_store.team_auth.poll(enabled, tx);
    // Slippy follows the session: started, stopped, or its account changed.
    #[cfg(target_os = "macos")]
    let changed = super::slippy::poll(ui, tx) | changed;
    changed
}

/// Turning the lab off cancels any work in progress; the saved session stays.
pub(in crate::rune_ui) fn settings_changed(ui: &mut Ui) {
    if !crate::labs::team_auth(&ui.settings) {
        ui.lab_store.team_auth.disable();
        #[cfg(target_os = "macos")]
        crate::slippy::sign_out();
    }
}

fn start(ui: &mut Ui, tx: &mut Tx, action: Action) {
    if ui.live() && crate::labs::team_auth(&ui.settings) && ui.lab_store.team_auth.start(action) {
        ui.lab_store.team_auth.checked = true;
        tx.wake_after(Duration::from_millis(100));
    }
}

fn open_browser(url: &Url) -> io::Result<()> {
    let status = Command::new("/usr/bin/open")
        .arg(url.as_str())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(io::Error::other("could not open the browser"));
    }
    Ok(())
}

pub(super) fn body(ui: &Ui, p: Paint) -> El {
    let status = |message: &str| {
        text(message)
            .key("team-auth-status")
            .font(controls::DESCRIPTION)
            .color(p.muted())
    };
    let mut strip = row().key("team-auth").gap(8.0).center_y().flex_wrap();
    if ui.lab_store.team_auth.job.is_some()
        && !matches!(ui.lab_store.team_auth.state, AuthState::SigningIn { .. })
    {
        return strip.child(status(tr("Checking team session…")));
    }
    let sign_in = || {
        controls::button(
            p,
            "team-auth-sign-in",
            tr("Sign in with Google"),
            ButtonKind::Secondary,
            |ui, tx| start(ui, tx, Action::SignIn),
        )
    };
    match &ui.lab_store.team_auth.state {
        AuthState::SignedOut => strip = strip.child(sign_in()),
        AuthState::SigningIn { .. } => {
            strip = strip
                .child(status(tr("Finish sign-in in your browser…")))
                .child(controls::button(
                    p,
                    "team-auth-cancel",
                    tr("Cancel"),
                    ButtonKind::Secondary,
                    |ui, _| {
                        if let Some(job) = &ui.lab_store.team_auth.job {
                            job.cancel.store(true, Ordering::Release);
                        }
                    },
                ));
        }
        AuthState::SignedIn { email, .. } => {
            strip = strip
                .child(status(&tr_fill("Signed in as {}", email)))
                .child(controls::button(
                    p,
                    "team-auth-sign-out",
                    tr("Sign out"),
                    ButtonKind::Secondary,
                    |ui, tx| start(ui, tx, Action::SignOut),
                ));
        }
        AuthState::NeedsReauth { reason, .. } => {
            strip = strip.child(status(tr(reason))).child(sign_in());
        }
    }
    strip
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_means_no_worker_even_with_the_card_visible() {
        let settings = crate::app_settings::AppSettings::default();
        assert!(!settings.labs.team_auth);
        let mut card = Card::default();
        assert!(!card.poll(crate::labs::team_auth(&settings), &mut Tx::default()));
        assert!(card.job.is_none());
        assert!(!card.checked);
    }

    #[test]
    fn the_session_is_checked_at_launch_without_the_window() {
        let mut card = Card::default();
        // Nothing about the window or Labs being open decides it.
        assert!(card.check_due(true));
        assert!(!card.check_due(false));
        card.checked = true;
        assert!(!card.check_due(true));
        card.next_refresh = Some(now_ms().saturating_sub(1));
        assert!(card.check_due(true));
        card.next_refresh = Some(now_ms() + 60_000);
        assert!(!card.check_due(true));
    }

    #[test]
    fn signing_out_stops_slippy_at_once_and_expiry_needs_sign_in() {
        let mut card = Card {
            state: AuthState::SignedIn {
                email: "nick@dwa1.com".into(),
                member_sub: "member".into(),
                expires_at_ms: None,
            },
            checked: true,
            ..Card::default()
        };
        card.needs_reauth("someone-else", AuthError::Refresh);
        assert!(matches!(card.state, AuthState::SignedIn { .. }));
        card.needs_reauth("member", AuthError::Refresh);
        assert!(matches!(card.state, AuthState::NeedsReauth { .. }));
        assert!(card.signed_out());
    }
}
