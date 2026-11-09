//! A pack's pictures: PNG only, its size read from its header before anything is decoded, and
//! then decoded whole within that size (a damaged or truncated picture is rejected, not drawn
//! in part).

use crate::{Limits, Report};

/// Checks a PNG: its size (width, height), or `None` (reported).
pub fn check(path: &str, bytes: &[u8], limits: &Limits, report: &mut Report) -> Option<(u32, u32)> {
    match decode(bytes, limits) {
        Ok(size) => Some(size),
        Err(why) => {
            report.reject("picture.png", Some(path), why);
            None
        }
    }
}

/// Decodes a PNG within `limits`, discarding its pixels: its size.
pub fn decode(bytes: &[u8], limits: &Limits) -> Result<(u32, u32), String> {
    let mut reader = read(bytes, limits)?;
    let mut buf = vec![0; reader.output_buffer_size()];
    reader
        .next_frame(&mut buf)
        .map_err(|e| format!("its pixels cannot be read: {e}"))?;
    let info = reader.info();
    Ok((info.width, info.height))
}

/// A PNG's size, from its header, within `limits` (nothing decoded).
pub(crate) fn header(bytes: &[u8], limits: &Limits) -> Result<(u32, u32), String> {
    let reader = read(bytes, limits)?;
    Ok((reader.info().width, reader.info().height))
}

/// A PNG's reader, its header read and its size within `limits`.
fn read<'a>(bytes: &'a [u8], limits: &Limits) -> Result<png::Reader<&'a [u8]>, String> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("not a PNG".into());
    }
    let side = u64::from(limits.picture_side);
    let mut decoder = png::Decoder::new_with_limits(
        bytes,
        png::Limits {
            // Room for the largest picture allowed at 16 bits a channel, and no more.
            bytes: usize::try_from(side * side * 8).unwrap_or(usize::MAX),
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND);
    let reader = decoder
        .read_info()
        .map_err(|e| format!("its header cannot be read: {e}"))?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || w > limits.picture_side || h > limits.picture_side {
        return Err(format!(
            "{w} by {h} pixels: a picture is at most {} pixels a side",
            limits.picture_side
        ));
    }
    if reader.info().animation_control.is_some() {
        return Err("an animated PNG: a pack's pictures are still".into());
    }
    Ok(reader)
}

#[cfg(test)]
pub(crate) fn encode(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut writer = e.write_header().expect("header");
        let data: Vec<u8> = (0..w * h * 4).map(|i| (i % 251) as u8).collect();
        writer.write_image_data(&data).expect("data");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_read_whole_within_its_size() {
        let limits = Limits::default();
        assert_eq!(decode(&encode(32, 16), &limits), Ok((32, 16)));
        // Truncated: rejected, not drawn in part.
        let png = encode(64, 64);
        assert!(decode(&png[..png.len() / 2], &limits).is_err());
        assert!(decode(b"GIF89a", &limits).is_err());
    }

    #[test]
    fn a_picture_too_large_is_refused_from_its_header() {
        let limits = Limits {
            picture_side: 100,
            ..Limits::default()
        };
        let err = decode(&encode(101, 10), &limits).unwrap_err();
        assert!(err.contains("at most 100"), "{err}");
    }

    /// A header claiming a vast picture over a few bytes: refused before any is decoded.
    #[test]
    fn a_header_claiming_a_vast_picture_is_refused() {
        let mut png = encode(4, 4);
        // IHDR's width and height at bytes 16 to 24; its CRC left wrong is refused either way.
        png[16..20].copy_from_slice(&60_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&60_000u32.to_be_bytes());
        assert!(decode(&png, &Limits::default()).is_err());
    }
}
