use std::io::Cursor;

use super::client::{Attachment, Error, base64};

const LONG_EDGE: usize = 2560;
const UPLOAD_BYTES: usize = 4 * 1024 * 1024;
const DECODE_BYTES: usize = 128 * 1024 * 1024;

pub(super) fn attachment(name: &'static str, bytes: &[u8]) -> Result<Attachment, Error> {
    let png = downscale(bytes).map_err(|_| Error::Image)?;
    Ok(Attachment {
        name,
        mime_type: "image/png",
        base64: base64(&png),
    })
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
