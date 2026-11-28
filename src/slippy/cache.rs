//! Slippy's attachments on this Mac, so a thread opens without downloading
//! its images again: each attachment's full bytes (`.img`) and a small
//! thumbnail (`.thumb`), in an owner-only folder beside the notification
//! cursor. Attachments never change once stored, so nothing here expires;
//! the least recently used go once the folder passes [`CAP`]. An upload Hex
//! just sent waits as `.sent` until its thread names the file's id. The
//! worker deletes the folder when it stops: sign-out, the lab off, or
//! another account.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use super::client::{Detail, FILE_BYTES, Role, valid_file_id, valid_ts};
use super::sync::FileKey;

/// The folder's largest total size before the least recently used go.
pub(super) const CAP: u64 = 50 * 1024 * 1024;
const KINDS: [&str; 4] = ["img", "thumb", "sent", "detail"];

pub(super) struct DiskCache {
    dir: PathBuf,
    cap: u64,
}

impl DiskCache {
    pub(super) fn new(dir: PathBuf) -> Self {
        Self::with_cap(dir, CAP)
    }

    pub(super) fn with_cap(dir: PathBuf, cap: u64) -> Self {
        Self { dir, cap }
    }

    /// The attachment's thumbnail, made again from its full bytes when only
    /// those are here.
    pub(super) fn thumb(&self, ts: &str, file_id: &str) -> Option<Arc<[u8]>> {
        let thumb = self.path(ts, file_id, "thumb")?;
        if let Some(bytes) = read(&thumb) {
            return Some(bytes.into());
        }
        let full = read(&self.path(ts, file_id, "img")?)?;
        Some(match super::image::thumbnail(&full) {
            Some(small) => {
                if self.ensure() {
                    let _ = crate::durable::write_private(&thumb, &small);
                }
                small.into()
            }
            None => full.into(),
        })
    }

    /// The attachment's full bytes, for the preview.
    pub(super) fn full(&self, ts: &str, file_id: &str) -> Option<Arc<[u8]>> {
        read(&self.path(ts, file_id, "img")?).map(Arc::from)
    }

    /// Keeps a downloaded attachment and returns its thumbnail: `full`
    /// itself when ImageIO can't make one. Without a folder it only
    /// returns the thumbnail.
    pub(super) fn store(&self, ts: &str, file_id: &str, full: &[u8]) -> Arc<[u8]> {
        let thumb = super::image::thumbnail(full);
        if let (Some(img), Some(thumb_path)) = (
            self.path(ts, file_id, "img"),
            self.path(ts, file_id, "thumb"),
        ) && self.ensure()
        {
            let _ = crate::durable::write_private(&img, full);
            if let Some(thumb) = &thumb {
                let _ = crate::durable::write_private(&thumb_path, thumb);
            }
            self.prune();
        }
        thumb.unwrap_or_else(|| full.to_vec()).into()
    }

    /// Keeps the `index`th image Hex uploaded with task `ts` until its
    /// thread names the file's id ([`claim`](Self::claim)).
    pub(super) fn stage(&self, ts: &str, index: usize, full: &[u8]) {
        if let Some(path) = self.sent(ts, index)
            && self.ensure()
        {
            let _ = crate::durable::write_private(&path, full);
            self.prune();
        }
    }

    /// Files a staged upload under the id its thread gave it. Whether one
    /// was waiting.
    pub(super) fn claim(&self, ts: &str, index: usize, file_id: &str) -> bool {
        match (self.sent(ts, index), self.path(ts, file_id, "img")) {
            (Some(from), Some(to)) => fs::rename(from, to).is_ok(),
            _ => false,
        }
    }

    /// A thread's last cached detail JSON, so switching to it after a restart
    /// shows its messages and cached thumbnails before any network call.
    pub(super) fn detail(&self, ts: &str) -> Option<Detail> {
        let bytes = read(&self.detail_path(ts)?)?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Saves `detail` beside its attachments.
    pub(super) fn store_detail(&self, detail: &Detail) {
        let Some(path) = self.detail_path(&detail.task.ts) else {
            return;
        };
        let Ok(bytes) = serde_json::to_vec(detail) else {
            return;
        };
        if self.ensure() {
            let _ = crate::durable::write_private(&path, &bytes);
            self.prune();
        }
    }

    /// Claims any staged uploads belonging to `detail` and loads every
    /// thumbnail already present on disk. Never touches the network.
    pub(super) fn local_thumbs(&self, detail: &Detail) -> Vec<(FileKey, Arc<[u8]>)> {
        let is_image = |file: &&super::AttachmentMeta| file.mime_type.starts_with("image/");
        let sent: Vec<&str> = detail
            .messages
            .iter()
            .find(|message| message.role == Role::User)
            .into_iter()
            .flat_map(|message| message.files.iter().filter(is_image))
            .map(|file| file.id.as_str())
            .collect();
        let mut found = Vec::new();
        for file in detail
            .messages
            .iter()
            .flat_map(|message| &message.files)
            .filter(is_image)
        {
            if let Some(index) = sent.iter().position(|id| *id == file.id) {
                self.claim(&detail.task.ts, index, &file.id);
            }
            if let Some(thumb) = self.thumb(&detail.task.ts, &file.id) {
                found.push(((detail.task.ts.clone(), file.id.clone()), thumb));
            }
        }
        found
    }

    /// Whether every image attachment in `detail` already has a thumbnail on disk.
    pub(super) fn has_all_thumbs(&self, detail: &Detail) -> bool {
        detail
            .messages
            .iter()
            .flat_map(|message| &message.files)
            .filter(|file| file.mime_type.starts_with("image/"))
            .all(|file| {
                self.path(&detail.task.ts, &file.id, "thumb")
                    .is_some_and(|path| path.is_file())
                    || self
                        .path(&detail.task.ts, &file.id, "img")
                        .is_some_and(|path| path.is_file())
            })
    }

    pub(super) fn clear(&self) {
        let _ = fs::remove_dir_all(&self.dir);
    }

    fn detail_path(&self, ts: &str) -> Option<PathBuf> {
        valid_ts(ts).then(|| self.dir.join(format!("{}.detail", ts.replace('.', "_"))))
    }

    fn path(&self, ts: &str, file_id: &str, kind: &str) -> Option<PathBuf> {
        (valid_ts(ts) && valid_file_id(file_id)).then(|| {
            self.dir
                .join(format!("{}-{file_id}.{kind}", ts.replace('.', "_")))
        })
    }

    fn sent(&self, ts: &str, index: usize) -> Option<PathBuf> {
        valid_ts(ts).then(|| {
            self.dir
                .join(format!("{}-{index}.sent", ts.replace('.', "_")))
        })
    }

    fn ensure(&self) -> bool {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .is_ok()
            && fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700)).is_ok()
    }

    /// Removes the least recently used attachments, each with its
    /// thumbnail, until the folder fits [`CAP`].
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut groups: HashMap<String, (u64, SystemTime, Vec<PathBuf>)> = HashMap::new();
        let mut total = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            let Some((stem, kind)) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.rsplit_once('.'))
            else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !KINDS.contains(&kind) || !metadata.is_file() {
                continue;
            }
            let used = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let group =
                groups
                    .entry(stem.to_owned())
                    .or_insert((0, SystemTime::UNIX_EPOCH, Vec::new()));
            group.0 += metadata.len();
            group.1 = group.1.max(used);
            total += metadata.len();
            group.2.push(path);
        }
        if total <= self.cap {
            return;
        }
        let mut groups: Vec<_> = groups.into_values().collect();
        groups.sort_by_key(|(_, used, _)| *used);
        for (size, _, paths) in groups {
            if total <= self.cap {
                break;
            }
            for path in paths {
                let _ = fs::remove_file(path);
            }
            total = total.saturating_sub(size);
        }
    }
}

/// A cached file's bytes, at most [`FILE_BYTES`], marking it used now.
fn read(path: &Path) -> Option<Vec<u8>> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    (&file).take(FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.is_empty() || bytes.len() as u64 > FILE_BYTES {
        return None;
    }
    let _ = file.set_modified(SystemTime::now());
    Some(bytes)
}
