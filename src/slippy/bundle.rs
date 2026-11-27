use std::fs::File;
use std::io::Read;
use std::path::Path;

use super::client::{Attachment, Error};

pub(super) fn read(path: &Path) -> Result<(String, Attachment), Error> {
    let manifest = scrawl_capture::bundle::read_manifest(path).map_err(|_| Error::Image)?;
    let words = manifest.transcript.text;
    let text = if words.trim().is_empty() {
        "Annotation".into()
    } else {
        words
    };
    let first = manifest.keyframes.first().map(|frame| frame.file.as_str());
    let image = [Some(manifest.contact_sheet.as_str()), first]
        .into_iter()
        .flatten()
        .filter(|name| name.to_ascii_lowercase().ends_with(".png"))
        .find_map(|name| {
            let file = File::open(path.join(name)).ok()?;
            let mut bytes = Vec::new();
            let max = 128 * 1024 * 1024;
            file.take(max + 1).read_to_end(&mut bytes).ok()?;
            if bytes.len() as u64 > max {
                return None;
            }
            super::image::attachment(super::image::Kind::Annotation, &bytes).ok()
        })
        .ok_or(Error::Image)?;
    Ok((text, image))
}
