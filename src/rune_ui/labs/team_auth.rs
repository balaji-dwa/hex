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
use crate::team_auth::{AuthError, AuthState, Client, REFRESH_WINDOW_MS, RefreshOutcome, now_ms};

const RETRY_DELAY_MS: u64 = 15_000;

pub(in crate::rune_ui) struct Card {
    pub(in crate::rune_ui) state: AuthState,
    error: Option<AuthError>,
    checked: bool,
    next_refresh: Option<u64>,
    job: Option<Job>,
}

impl Default for Card {
    fn default() -> Self {
        Self {
            state: AuthState::SignedOut,
            error: None,
            checked: false,
            next_refresh: None,
            job: None,
        }
    }
}

struct Job {
    action: Action,
    cancel: Arc<AtomicBool>,
    result: Receiver<Result<RefreshOutcome, AuthError>>,
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
    fn run(self, cancel: &AtomicBool) -> Result<RefreshOutcome, AuthError> {
        if cancel.load(Ordering::Acquire) {
            return Err(AuthError::Cancelled);
        }
        let client = Client::configured()?;
        match self {
            Self::Check => client.refresh_if_needed(cancel),
            Self::SignIn => client
                .sign_in(cancel, open_browser)
                .map(RefreshOutcome::Complete),
            Self::SignOut => client.sign_out().map(RefreshOutcome::Complete),
        }
    }
}

impl Card {
    pub(in crate::rune_ui) fn disable(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Release);
        }
        self.state = AuthState::SignedOut;
        self.error = None;
        self.checked = false;
        self.next_refresh = None;
    }

    #[cfg(target_os = "macos")]
    pub(in crate::rune_ui) fn needs_reauth(&mut self, member: &str, error: AuthError) {
        if matches!(&self.state, AuthState::SignedIn { member_sub, .. } if member_sub == member)
            && let Some(state) = self.state.reauth(error)
        {
            self.state = state;
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
        self.error = None;
        if started.is_err() {
            self.complete(action, Err(AuthError::Service));
            return true;
        }
        self.job = Some(Job {
            action,
            cancel,
            result,
        });
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

    fn complete(&mut self, action: Action, result: Result<RefreshOutcome, AuthError>) {
        let now = now_ms();
        match result {
            Ok(RefreshOutcome::Complete(state)) => {
                self.next_refresh = match state {
                    AuthState::SignedIn {
                        expires_at_ms: Some(expires),
                        ..
                    } => {
                        let early = expires.saturating_sub(REFRESH_WINDOW_MS);
                        Some(if early > now { early } else { expires })
                    }
                    _ => None,
                };
                self.state = state;
            }
            Ok(RefreshOutcome::Retry(state)) => {
                self.state = state;
                self.next_refresh = Some(now + RETRY_DELAY_MS);
            }
            Err(error) => {
                if let Some(state) = self.state.reauth(error) {
                    self.state = state;
                    self.next_refresh = None;
                } else {
                    if matches!(self.state, AuthState::SigningIn { .. }) {
                        self.state = AuthState::SignedOut;
                    }
                    if error != AuthError::Cancelled {
                        self.error = Some(error);
                    }
                    if matches!(action, Action::Check) {
                        self.next_refresh = Some(now + RETRY_DELAY_MS);
                    }
                }
            }
        }
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
            let action = job.action;
            let stale_check = matches!(action, Action::Check) && job.cancel.load(Ordering::Acquire);
            if enabled && self.checked && !stale_check {
                self.complete(action, result);
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
        AuthState::SignedOut => {
            if let Some(error) = ui.lab_store.team_auth.error {
                strip = strip.child(status(tr(error.message())));
            }
            strip = strip.child(sign_in());
        }
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

    fn signed_in() -> AuthState {
        AuthState::SignedIn {
            email: "nick@dwa1.com".into(),
            member_sub: "member".into(),
            expires_at_ms: Some(0),
        }
    }

    fn finish(card: &mut Card, action: Action, result: Result<RefreshOutcome, AuthError>) {
        assert!(card.job.is_none());
        let (sender, result_rx) = mpsc::channel();
        sender.send(result).unwrap();
        card.checked = true;
        card.job = Some(Job {
            action,
            cancel: Arc::new(AtomicBool::new(false)),
            result: result_rx,
        });
        assert!(card.poll(true, &mut Tx::default()));
    }

    #[test]
    fn retry_keeps_the_slippy_member_and_backs_off_even_with_an_expired_token() {
        use crate::rune_ui::{Service, Ui};

        let mut settings = crate::app_settings::AppSettings::default();
        settings.labs.team_auth = true;
        let mut ui = Ui::with(settings, Service::Fixture(Vec::new()));
        let before = now_ms();
        finish(
            &mut ui.lab_store.team_auth,
            Action::Check,
            Ok(RefreshOutcome::Retry(signed_in())),
        );
        let card = &mut ui.lab_store.team_auth;
        let retry = card.next_refresh.unwrap();
        assert!((before + RETRY_DELAY_MS..=now_ms() + RETRY_DELAY_MS).contains(&retry));
        assert!(!card.check_due(true));
        assert!(!card.signed_out());
        assert_eq!(super::super::slippy::member(&ui), Some("member"));

        let card = &mut ui.lab_store.team_auth;
        finish(card, Action::Check, Err(AuthError::Cancelled));
        assert_eq!(card.state, signed_in());
        assert!(!card.check_due(true));
        card.needs_reauth("member", AuthError::Service);
        assert_eq!(card.state, signed_in());
        assert!(card.next_refresh.is_some());
    }

    #[test]
    fn a_rejected_member_stops_slippy_and_a_stale_check_cannot_restore_it() {
        let mut card = Card {
            state: signed_in(),
            checked: true,
            ..Card::default()
        };
        let (sender, result) = mpsc::channel();
        sender.send(Ok(RefreshOutcome::Retry(signed_in()))).unwrap();
        card.job = Some(Job {
            action: Action::Check,
            cancel: Arc::new(AtomicBool::new(false)),
            result,
        });

        card.needs_reauth("someone-else", AuthError::Refresh);
        assert!(matches!(card.state, AuthState::SignedIn { .. }));
        card.needs_reauth("member", AuthError::Refresh);
        assert!(card.poll(true, &mut Tx::default()));
        assert!(matches!(card.state, AuthState::NeedsReauth { .. }));
        assert!(card.signed_out());
        assert!(card.next_refresh.is_none());
    }

    #[test]
    fn explicit_sign_in_distinguishes_a_denial_from_an_outage() {
        let mut card = Card {
            state: AuthState::SigningIn { started_at_ms: 0 },
            ..Card::default()
        };
        finish(&mut card, Action::SignIn, Err(AuthError::Service));
        assert_eq!(card.state, AuthState::SignedOut);
        assert_eq!(card.error, Some(AuthError::Service));
        assert!(card.next_refresh.is_none());

        card.state = AuthState::SigningIn { started_at_ms: 0 };
        finish(&mut card, Action::SignIn, Err(AuthError::Denied));
        assert!(matches!(card.state, AuthState::NeedsReauth { .. }));
        assert!(card.next_refresh.is_none());
    }
}
