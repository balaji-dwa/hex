pub(crate) mod capsule;
mod dictation_pill;
mod pane;

#[cfg(test)]
mod tests;

pub(crate) use dictation_pill::{install, offer_dictation};
pub(in crate::rune_ui) use pane::{Data, open_link, poll, view};

use std::sync::mpsc::{Receiver, TryRecvError};

use rune::{El, Tx};
use rune_kit::{Button, ButtonVariant, ControlSize};

use crate::desktop::i18n::tr;
use crate::history::{HistoryEntry, HistoryKind};
use crate::rune_ui::{Ui, controls::Paint};
use crate::slippy::{self, Error, Task};
use crate::team_auth::AuthState;

pub(in crate::rune_ui) fn member(ui: &Ui) -> Option<&str> {
    if ui.platform.caps.os == crate::rune_ui::platform::Os::Macos
        && crate::labs::team_auth(&ui.settings)
        && let AuthState::SignedIn {
            member_sub, email, ..
        } = &ui.lab_store.team_auth.state
        && email.rsplit_once('@').is_some_and(|(name, domain)| {
            !name.is_empty() && domain.eq_ignore_ascii_case("dwa1.com")
        })
    {
        Some(member_sub)
    } else {
        None
    }
}

pub(in crate::rune_ui) fn visible(ui: &Ui) -> bool {
    member(ui).is_some()
}

/// Whether the capture capsule and the screenshot editor offer Slippy: its
/// worker runs only for a signed-in team member with the lab on.
pub(crate) fn offered() -> bool {
    slippy::active()
}

pub(in crate::rune_ui) fn unread(ui: &Ui) -> bool {
    !ui.lab_store.slippy.snapshot.unread.is_empty()
}

pub(crate) const SEND: &str = "Send to Slippy";
pub(crate) const SENT: &str = "Sent to Slippy";
pub(crate) const SENDING: &str = "Sending to Slippy…";
pub(crate) const TRANSCRIPT: &str = "Sends this transcript to Slippy";

/// The screenshot editor's send: `png` and `words`, waiting on Slippy's
/// answer. Runs on the editor's export thread.
pub(crate) fn send_image(origin: slippy::Origin, png: Vec<u8>, words: &str) -> anyhow::Result<()> {
    slippy::send_image(origin, png, words.to_owned())
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!(error.message()))
}

pub(crate) fn result(receiver: &Receiver<Result<Task, Error>>) -> Option<Result<Task, Error>> {
    match receiver.try_recv() {
        Ok(result) => Some(result),
        Err(TryRecvError::Disconnected) => Some(Err(Error::Unavailable)),
        Err(TryRecvError::Empty) => None,
    }
}

pub(in crate::rune_ui) fn history_action(ui: &Ui, p: Paint, entry: &HistoryEntry) -> Option<El> {
    if !visible(ui)
        || !matches!(
            entry.kind,
            HistoryKind::Dictation | HistoryKind::Send | HistoryKind::Raw
        )
        || entry.final_text.trim().is_empty()
    {
        return None;
    }
    let id = entry.id;
    let words = entry.final_text.clone();
    let progress = ui
        .lab_store
        .slippy
        .history
        .as_ref()
        .filter(|(row, _)| *row == id)
        .map(|(_, status)| status);
    let label = match progress {
        Some(Progress::Pending(_)) => SENDING,
        Some(Progress::Finished(Ok(_), _)) => SENT,
        Some(Progress::Finished(Err(error), _)) => error.message(),
        None => SEND,
    };
    Some(
        Button::new(tr(label))
            .help(tr(TRANSCRIPT))
            .on_press_tx(move |ui: &mut Ui, tx: &mut Tx| {
                if !visible(ui)
                    || ui
                        .lab_store
                        .slippy
                        .history
                        .as_ref()
                        .is_some_and(|(_, status)| status.pending())
                {
                    return;
                }
                ui.lab_store.slippy.history = Some((
                    id,
                    Progress::start(
                        slippy::handle().and_then(|handle| handle.dictation(words.clone())),
                    ),
                ));
                tx.wake_after(pane::RESULT_TICK);
            })
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .disabled(progress.is_some_and(Progress::pending))
            .build(p.k)
            .key("row-send-slippy"),
    )
}

enum Progress {
    Pending(Receiver<Result<Task, Error>>),
    Finished(Result<Task, Error>, std::time::Instant),
}

impl Progress {
    fn start(response: Result<Receiver<Result<Task, Error>>, Error>) -> Self {
        match response {
            Ok(receiver) => Self::Pending(receiver),
            Err(error) => Self::Finished(Err(error), std::time::Instant::now()),
        }
    }

    fn pending(&self) -> bool {
        matches!(self, Self::Pending(_))
    }
}

pub(in crate::rune_ui) fn parse_link(url: &url::Url) -> Result<String, String> {
    url.path()
        .strip_prefix('/')
        .filter(|ts| {
            slippy::valid_ts(ts)
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none()
        })
        .map(str::to_owned)
        .ok_or_else(|| "the link names no valid Slippy task".into())
}
