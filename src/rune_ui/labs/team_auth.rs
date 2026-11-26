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
        if matches!(action, Action::SignIn) {
            self.state = AuthState::SigningIn {
                started_at_ms: now_ms(),
            };
        }
        true
    }

    fn poll(&mut self, enabled: bool, visible: bool, tx: &mut Tx) -> bool {
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
        if enabled
            && visible
            && (!self.checked || self.next_refresh.is_some_and(|at| now_ms() >= at))
        {
            self.checked = true;
            self.next_refresh = None;
            changed |= self.start(Action::Check);
            tx.wake_after(Duration::from_millis(100));
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
    let visible = ui.labs_open && ui.window.as_ref().is_some_and(rune::AppHandle::is_visible);
    ui.lab_store.team_auth.poll(enabled, visible, tx)
}

/// Turning the lab off cancels any work in progress; the saved session stays.
pub(in crate::rune_ui) fn settings_changed(ui: &mut Ui) {
    if !crate::labs::team_auth(&ui.settings) {
        ui.lab_store.team_auth.disable();
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
        assert!(!card.poll(crate::labs::team_auth(&settings), true, &mut Tx::default()));
        assert!(card.job.is_none());
        assert!(!card.checked);
    }
}
