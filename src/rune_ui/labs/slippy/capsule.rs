//! The finished-annotation capsule's Slippy button: its label, and the send
//! it starts. Kept apart from the capsule's OpenCode sends, so it never
//! enters Undo or an `Always…` rule.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::Receiver;

use super::{SENT, result};
use crate::rune_ui::capture_capsule::flow::{Done, Pres};
use crate::slippy::{self, Error, Task};

const IDLE: &str = "Slippy";
const SENDING: &str = "Sending…";
const FAILED: &str = "Couldn't send";

/// The latest capsule send, keyed by its annotation bundle.
static SEND: Mutex<Option<(PathBuf, Send)>> = Mutex::new(None);

enum Send {
    Pending(Receiver<Result<Task, Error>>),
    Finished(Result<(), Error>),
}

/// Shows the Slippy button on a finished capsule for `bundle` when Slippy is
/// signed in; hides it otherwise.
pub(crate) fn decorate(pres: &mut Pres, bundle: Option<&PathBuf>) {
    let Pres::Done(done) = pres else {
        return;
    };
    let state = SEND.lock().unwrap_or_else(|error| error.into_inner());
    let progress = state
        .as_ref()
        .filter(|(sent, _)| Some(sent) == bundle)
        .map(|(_, send)| send);
    apply(
        done,
        super::offered(),
        bundle.map(PathBuf::as_path),
        progress,
    );
}

fn apply(done: &mut Done, active: bool, bundle: Option<&Path>, progress: Option<&Send>) {
    done.slippy =
        (active && bundle.is_some() && done.open.is_none() && done.always_popover.is_none())
            .then(|| label(progress));
}

fn label(progress: Option<&Send>) -> &'static str {
    match progress {
        None => IDLE,
        Some(Send::Pending(_)) => SENDING,
        Some(Send::Finished(Ok(()))) => SENT,
        Some(Send::Finished(Err(_))) => FAILED,
    }
}

/// Sends `bundle`'s image and words to Slippy on its worker thread. Runs on
/// the main thread: reading and shrinking the bundle happen on the worker.
pub(crate) fn send(bundle: PathBuf) {
    let mut state = SEND.lock().unwrap_or_else(|error| error.into_inner());
    if state
        .as_ref()
        .is_some_and(|(sent, send)| *sent == bundle && !matches!(send, Send::Finished(Err(_))))
    {
        return;
    }
    let send = match slippy::send_annotation_bundle(bundle.clone()) {
        Ok(receiver) => Send::Pending(receiver),
        Err(error) => Send::Finished(Err(error)),
    };
    *state = Some((bundle, send));
}

/// Whether a send is in flight; a finished one updates its label.
pub(crate) fn pending() -> bool {
    let mut state = SEND.lock().unwrap_or_else(|error| error.into_inner());
    let Some((_, send)) = state.as_mut() else {
        return false;
    };
    if let Send::Pending(receiver) = send {
        match result(receiver) {
            Some(answer) => *send = Send::Finished(answer.map(|_| ())),
            None => return true,
        }
    }
    false
}

#[cfg(test)]
pub(super) fn test_apply(done: &mut Done, active: bool, bundle: Option<&Path>) {
    apply(done, active, bundle, None);
}

#[cfg(test)]
pub(super) fn test_label(pending: bool, finished: Option<bool>) -> &'static str {
    let (_sender, receiver) = std::sync::mpsc::channel();
    let send = match (pending, finished) {
        (true, _) => Some(Send::Pending(receiver)),
        (false, Some(true)) => Some(Send::Finished(Ok(()))),
        (false, Some(false)) => Some(Send::Finished(Err(Error::Unavailable))),
        (false, None) => None,
    };
    label(send.as_ref())
}
