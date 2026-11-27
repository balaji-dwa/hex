use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::team_auth::{AuthError, Client as AuthClient};

const TASKS: &str = "/api/hex/tasks";
const JSON_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const FILE_BYTES: u64 = 12 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Origin {
    Dictation,
    Screenshot,
    Annotate,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Queued,
    Running,
    Waiting,
    Done,
    Failed,
}

impl Status {
    pub(crate) fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Waiting => "Waiting",
            Self::Done => "Done",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Task {
    pub ts: String,
    pub origin: Origin,
    pub title: String,
    pub status: Status,
    pub seq: u64,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentMeta {
    pub id: String,
    pub name: String,
    pub mime_type: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct Message {
    pub ts: String,
    pub role: Role,
    pub text: String,
    pub files: Vec<AttachmentMeta>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Attachment {
    pub name: &'static str,
    pub mime_type: &'static str,
    pub base64: String,
    /// The bytes `base64` encodes, kept to seed this Mac's cache.
    #[serde(skip)]
    pub raw: Vec<u8>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Create<'a> {
    idempotency_key: String,
    origin: Origin,
    text: &'a str,
    files: &'a [Attachment],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Reply<'a> {
    idempotency_key: String,
    text: &'a str,
    files: &'a [Attachment],
}

#[derive(Deserialize)]
pub(super) struct Created {
    pub task: Task,
    #[cfg(test)]
    pub deduplicated: bool,
}

#[derive(Deserialize)]
pub(super) struct Listed {
    pub cursor: u64,
    pub tasks: Vec<Task>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct Detail {
    pub task: Task,
    pub messages: Vec<Message>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Replied {
    pub task: Task,
    #[cfg(test)]
    pub message_ts: String,
    #[cfg(test)]
    pub deduplicated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Error {
    Auth(AuthError),
    Text,
    Image,
    Response,
    Unavailable,
    Rejected(u16),
}

impl Error {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Auth(AuthError::Refresh) => AuthError::Refresh.message(),
            Self::Text => "Enter between 1 and 12,000 characters.",
            Self::Image => "Couldn't prepare the image for Slippy.",
            Self::Rejected(404) => "Slippy task not found.",
            Self::Rejected(429) => "Too many open Slippy tasks.",
            _ => "Couldn't reach Slippy. Try again.",
        }
    }
}

impl From<AuthError> for Error {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

pub(super) struct Client<'a> {
    pub auth: &'a AuthClient,
    pub member: &'a str,
    pub cancel: &'a AtomicBool,
}

impl Client<'_> {
    pub(super) fn create(
        &self,
        origin: Origin,
        text: &str,
        files: &[Attachment],
    ) -> Result<Created, Error> {
        let body = Create {
            idempotency_key: idempotency_key()?,
            origin,
            text: request_text(text)?,
            files,
        };
        self.post(TASKS, &body)
    }

    pub(super) fn list(&self, since: u64, limit: u8) -> Result<Listed, Error> {
        self.request(&format!("{TASKS}?since={since}&limit={limit}"), None)
    }

    pub(super) fn detail(&self, ts: &str) -> Result<Detail, Error> {
        self.request(&task_path(ts)?, None)
    }

    pub(super) fn reply(&self, ts: &str, text: &str) -> Result<Replied, Error> {
        let body = Reply {
            idempotency_key: idempotency_key()?,
            text: request_text(text)?,
            files: &[],
        };
        self.post(&format!("{}/messages", task_path(ts)?), &body)
    }

    /// An attachment's bytes and their `Content-Type`, at most
    /// [`FILE_BYTES`].
    pub(super) fn file(&self, ts: &str, file_id: &str) -> Result<(String, Vec<u8>), Error> {
        if !valid_file_id(file_id) {
            return Err(Error::Rejected(404));
        }
        let path = format!("{}/files/{file_id}", task_path(ts)?);
        self.fetch(&path, None, FILE_BYTES)
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T, Error> {
        let body = serde_json::to_vec(body).map_err(|_| Error::Response)?;
        self.request(path, Some(&body))
    }

    fn request<T: DeserializeOwned>(&self, path: &str, body: Option<&[u8]>) -> Result<T, Error> {
        let (_, bytes) = self.fetch(path, body, JSON_BYTES)?;
        serde_json::from_slice(&bytes).map_err(|_| Error::Response)
    }

    fn fetch(
        &self,
        path: &str,
        body: Option<&[u8]>,
        limit: u64,
    ) -> Result<(String, Vec<u8>), Error> {
        let mut response = self
            .auth
            .send_authorized(self.member, path, body, self.cancel)?;
        if !response.status().is_success() {
            return Err(Error::Rejected(response.status().as_u16()));
        }
        let mime = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|_| Error::Response)?;
        Ok((mime, bytes))
    }
}

pub(crate) fn valid_ts(ts: &str) -> bool {
    ts.split_once('.').is_some_and(|(seconds, micros)| {
        (10..=12).contains(&seconds.len())
            && micros.len() == 6
            && seconds
                .bytes()
                .chain(micros.bytes())
                .all(|byte| byte.is_ascii_digit())
    })
}

pub(super) fn valid_file_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn task_path(ts: &str) -> Result<String, Error> {
    valid_ts(ts)
        .then(|| format!("{TASKS}/{ts}"))
        .ok_or(Error::Rejected(404))
}

fn request_text(text: &str) -> Result<&str, Error> {
    let text = text.trim();
    if text.is_empty() || text.encode_utf16().count() > 12_000 {
        return Err(Error::Text);
    }
    Ok(text)
}

fn idempotency_key() -> Result<String, Error> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::Unavailable)?;
    use std::fmt::Write;
    let mut key = String::with_capacity(32);
    for byte in bytes {
        write!(key, "{byte:02x}").unwrap();
    }
    Ok(key)
}

pub(super) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = u32::from_be_bytes([
            0,
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ]);
        let symbol = |shift: u32| char::from(ALPHABET[((value >> shift) & 63) as usize]);
        out.extend([
            symbol(18),
            symbol(12),
            if chunk.len() > 1 { symbol(6) } else { '=' },
            if chunk.len() > 2 { symbol(0) } else { '=' },
        ]);
    }
    out
}
