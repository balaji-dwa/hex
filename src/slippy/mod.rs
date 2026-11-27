mod bundle;
mod client;
mod image;
pub(crate) mod notify;
mod sync;

#[cfg(test)]
mod tests;

pub(crate) use client::{AttachmentMeta, Detail, Error, Origin, Role, Status, Task, valid_ts};
pub(crate) use sync::{FileKey, Snapshot, active, handle, reconcile, sign_out, snapshot};

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

/// Sends an edited screenshot (or a recording's contact sheet) and its
/// spoken words, waiting for Slippy's answer. Call it off the main thread.
pub(crate) fn send_image(origin: Origin, png: Vec<u8>, words: String) -> Result<Task, Error> {
    handle()?
        .image(origin, png, words)?
        .recv()
        .unwrap_or(Err(Error::Unavailable))
}

pub(crate) fn send_annotation_bundle(
    path: PathBuf,
) -> Result<Receiver<Result<Task, Error>>, Error> {
    handle()?.submit(move || {
        let (text, attachment) = bundle::read(&path)?;
        Ok((Origin::Annotate, text, vec![attachment]))
    })
}
