use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_SESSION_BYTES: u64 = 64 * 1024;

#[derive(Deserialize, Serialize)]
pub(super) struct StoredSession {
    pub origin: String,
    pub client_id: String,
    pub token_endpoint: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub access_expires_at_ms: Option<u64>,
    pub member_sub: String,
    pub email: String,
    // DCR servers can require the identical URI on subsequent sign-ins.
    #[serde(default)]
    pub redirect_port: Option<u16>,
}

impl fmt::Debug for StoredSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredSession")
            .field("access_token", &"[redacted]")
            .field("refresh_token", &"[redacted]")
            .finish_non_exhaustive()
    }
}

pub(super) fn path() -> io::Result<PathBuf> {
    let isolated = std::env::var_os("HEX_APPLICATION_SUPPORT_DIR");
    session_path(
        isolated.as_deref(),
        crate::app_paths::data_dir().as_deref(),
        crate::app_paths::is_macos_test_build(),
    )
}

pub(super) fn session_path(
    isolated: Option<&OsStr>,
    data_dir: Option<&Path>,
    test_build: bool,
) -> io::Result<PathBuf> {
    if let Some(isolated) = isolated {
        if isolated.is_empty() {
            return Err(io::Error::other("the isolated data directory is empty"));
        }
        return Ok(Path::new(isolated).join("hex-auth-session.json"));
    }
    let directory = if test_build {
        "hex-auth-test"
    } else {
        "hex-auth"
    };
    Ok(data_dir
        .ok_or_else(|| io::Error::other("application data directory is unavailable"))?
        .join(directory)
        .join("session.json"))
}

pub(super) fn load(path: &Path) -> io::Result<Option<StoredSession>> {
    let Some(metadata) = file_metadata(path)? else {
        return Ok(None);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(io::Error::other("the team session is not private"));
        }
    }
    let mut data = Vec::new();
    File::open(path)?
        .take(MAX_SESSION_BYTES + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 > MAX_SESSION_BYTES {
        return Err(io::Error::other("the team session is too large"));
    }
    serde_json::from_slice(&data)
        .map(Some)
        .map_err(|_| io::Error::other("the team session is unreadable"))
}

pub(super) fn save(path: &Path, session: &StoredSession) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("missing session directory"))?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    if !fs::symlink_metadata(directory)?.is_dir() {
        return Err(io::Error::other("the session directory is not a directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    file_metadata(path)?;
    let data = serde_json::to_vec(session)
        .map_err(|_| io::Error::other("could not serialize the team session"))?;
    crate::durable::write_private(path, &data)
}

pub(super) fn clear_session(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => crate::durable::sync_directory(path.parent().unwrap_or(Path::new("."))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn file_metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata)),
        Ok(_) => Err(io::Error::other("the team session is not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
