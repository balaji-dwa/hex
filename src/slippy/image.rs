//! Slippy's images: uploads re-encoded as JPEG through ImageIO
//! (`native/slippy_image.m`), the tiny thumbnails the thread shows, and the
//! PNG downscale kept for an image ImageIO can't read.

use std::io::Cursor;

use super::client::{Attachment, Error, base64};

const LONG_EDGE: usize = 2560;
const UPLOAD_BYTES: usize = 4 * 1024 * 1024;
const DECODE_BYTES: usize = 128 * 1024 * 1024;
/// An upload's JPEG: its long edge and quality, then a smaller retry for one
/// still over [`JPEG_TARGET`].
pub(super) const JPEG_EDGE: u32 = 2048;
const JPEG_QUALITY: f32 = 0.80;
const JPEG_TARGET: usize = 2 * 1024 * 1024;
const RETRY_EDGE: u32 = 1600;
const RETRY_QUALITY: f32 = 0.65;
/// A thumbnail's short edge in pixels: the thread crops it to a 64-point
/// square, sharp at 2x. Its long edge stops at [`THUMB_MAX`] for a panorama.
pub(super) const THUMB_SHORT: u32 = 128;
const THUMB_MAX: u32 = 512;
const THUMB_QUALITY: f32 = 0.78;

unsafe extern "C" {
    fn hex_slippy_encode_jpeg(
        input: *const u8,
        input_len: usize,
        max_edge: u32,
        quality: f32,
        out_buf: *mut *mut u8,
        out_len: *mut usize,
    ) -> i32;
    fn hex_slippy_free_bytes(buffer: *mut u8);
    fn hex_slippy_image_size(
        input: *const u8,
        input_len: usize,
        width: *mut u32,
        height: *mut u32,
    ) -> i32;
}

/// Which capture an upload is, for its file name.
#[derive(Clone, Copy)]
pub(super) enum Kind {
    Screenshot,
    Annotation,
}

impl Kind {
    fn name(self, jpeg: bool) -> &'static str {
        match (self, jpeg) {
            (Self::Screenshot, true) => "screenshot.jpg",
            (Self::Screenshot, false) => "screenshot.png",
            (Self::Annotation, true) => "annotation.jpg",
            (Self::Annotation, false) => "annotation.png",
        }
    }
}

/// The upload for `bytes`: a JPEG at most [`JPEG_EDGE`] pixels long, or,
/// when ImageIO can't read it, the PNG downscaled to fit.
pub(super) fn attachment(kind: Kind, bytes: &[u8]) -> Result<Attachment, Error> {
    let (jpeg, raw) = match upload_jpeg(bytes) {
        Some(jpeg) => (true, jpeg),
        None => (false, downscale(bytes).map_err(|_| Error::Image)?),
    };
    Ok(Attachment {
        name: kind.name(jpeg),
        mime_type: if jpeg { "image/jpeg" } else { "image/png" },
        base64: base64(&raw),
        raw,
    })
}

fn upload_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    let first = jpeg(bytes, JPEG_EDGE, JPEG_QUALITY)?;
    if first.len() <= JPEG_TARGET {
        return Some(first);
    }
    let mut edge = RETRY_EDGE;
    loop {
        let retry = jpeg(bytes, edge, RETRY_QUALITY)?;
        if retry.len() <= UPLOAD_BYTES || edge <= 256 {
            return (retry.len() <= UPLOAD_BYTES).then_some(retry);
        }
        edge = edge * 3 / 4;
    }
}

/// A thumbnail of any image ImageIO reads: a small JPEG whose short edge
/// is [`THUMB_SHORT`] pixels (or the image's own, if smaller).
pub(super) fn thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    let (width, height) = size(bytes)?;
    let (long, short) = (width.max(height), width.min(height));
    let edge = (u64::from(THUMB_SHORT) * u64::from(long)).div_ceil(u64::from(short));
    let edge = u32::try_from(edge)
        .unwrap_or(u32::MAX)
        .min(THUMB_MAX)
        .min(long);
    jpeg(bytes, edge, THUMB_QUALITY)
}

/// The pixel size of the image in `bytes`, as shown (its orientation
/// applied), from its header.
pub(super) fn size(bytes: &[u8]) -> Option<(u32, u32)> {
    let (mut width, mut height) = (0, 0);
    // SAFETY: `bytes` is live for the call, which only reads it and writes
    // the two sizes.
    let ok = unsafe { hex_slippy_image_size(bytes.as_ptr(), bytes.len(), &mut width, &mut height) };
    (ok != 0).then_some((width, height))
}

/// `bytes` decoded, scaled to at most `max_edge` pixels long, and encoded
/// as a JPEG at `quality`.
pub(super) fn jpeg(bytes: &[u8], max_edge: u32, quality: f32) -> Option<Vec<u8>> {
    if bytes.is_empty() {
        return None;
    }
    let mut buffer = std::ptr::null_mut();
    let mut len = 0;
    // SAFETY: `bytes` is live for the call, which only reads it; the shim
    // writes a malloc'ed buffer and its length on success.
    let ok = unsafe {
        hex_slippy_encode_jpeg(
            bytes.as_ptr(),
            bytes.len(),
            max_edge,
            quality,
            &mut buffer,
            &mut len,
        )
    };
    if ok == 0 || buffer.is_null() {
        return None;
    }
    // SAFETY: on success `buffer` holds `len` initialized bytes until freed.
    let output = unsafe { std::slice::from_raw_parts(buffer, len) }.to_vec();
    // SAFETY: the shim's own buffer, freed once.
    unsafe { hex_slippy_free_bytes(buffer) };
    Some(output)
}

pub(super) fn downscale(bytes: &[u8]) -> Result<Vec<u8>, ()> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: DECODE_BYTES,
        },
    );
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|_| ())?;
    let size = reader
        .output_buffer_size()
        .filter(|size| *size <= DECODE_BYTES)
        .ok_or(())?;
    let mut pixels = vec![0; size];
    let info = reader.next_frame(&mut pixels).map_err(|_| ())?;
    let width = info.width as usize;
    let height = info.height as usize;
    let long = width.max(height);
    if long <= LONG_EDGE && bytes.len() <= UPLOAD_BYTES {
        return Ok(bytes.to_vec());
    }
    let channels = info.color_type.samples();
    let mut edge = long.min(LONG_EDGE);
    loop {
        let w = (width * edge / long).max(1);
        let h = (height * edge / long).max(1);
        let scaled = box_filter(&pixels, (width, height), (w, h), channels);
        let mut output = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut output, w as u32, h as u32);
            encoder.set_color(info.color_type);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .and_then(|mut writer| writer.write_image_data(&scaled))
                .map_err(|_| ())?;
        }
        if output.len() <= UPLOAD_BYTES {
            return Ok(output);
        }
        edge = edge * 3 / 4;
    }
}

fn box_filter(
    pixels: &[u8],
    (width, height): (usize, usize),
    (out_width, out_height): (usize, usize),
    channels: usize,
) -> Vec<u8> {
    let mut output = Vec::with_capacity(out_width * out_height * channels);
    let alpha = channels.is_multiple_of(2);
    for y in 0..out_height {
        let top = y * height / out_height;
        let bottom = (y + 1) * height / out_height;
        for x in 0..out_width {
            let left = x * width / out_width;
            let right = (x + 1) * width / out_width;
            let mut totals = [0_u64; 4];
            for sy in top..bottom {
                let offset = (sy * width + left) * channels;
                for sample in
                    pixels[offset..offset + (right - left) * channels].chunks_exact(channels)
                {
                    let weight = if alpha {
                        u64::from(sample[channels - 1])
                    } else {
                        1
                    };
                    for (index, &channel) in sample.iter().enumerate() {
                        totals[index] += u64::from(channel)
                            * if alpha && index == channels - 1 {
                                1
                            } else {
                                weight
                            };
                    }
                }
            }
            let count = ((right - left) * (bottom - top)) as u64;
            let weight = if alpha { totals[channels - 1] } else { count };
            for (index, total) in totals.into_iter().take(channels).enumerate() {
                let divisor = if alpha && index == channels - 1 {
                    count
                } else {
                    weight
                };
                output.push(
                    (total + divisor / 2)
                        .checked_div(divisor)
                        .map_or(0, |value| value as u8),
                );
            }
        }
    }
    output
}
