//! Attachment preparation happens before content addressing, never during replay.
use crate::error::invalid;
use anyhow::Result;
use image::{AnimationDecoder, ImageDecoder, ImageFormat, ImageReader};
use std::io::Cursor;

pub const DEFAULT_MAX_DIMENSION: u32 = 4096;

pub fn max_dimension(eff: &crate::settings::Effective) -> u32 {
    eff.get("attachments.max_dimension")
        .and_then(|v| v.as_integer())
        .filter(|n| *n >= 0)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(DEFAULT_MAX_DIMENSION)
}

/// Strip private metadata without recompressing pixels. Decode only to resize or apply EXIF
/// orientation. A zero dimension disables resizing, never metadata stripping.
pub fn prepare(data: &[u8], max: u32) -> Result<Vec<u8>> {
    let Ok(format) = image::guess_format(data) else {
        return Ok(data.to_vec());
    };
    let (clean, oriented) = match format {
        ImageFormat::Jpeg => (strip_jpeg(data)?, data.windows(6).any(|w| w == b"Exif\0\0")),
        ImageFormat::Png => (strip_png(data)?, data.windows(4).any(|w| w == b"eXIf")),
        ImageFormat::WebP => (strip_webp(data)?, data.windows(4).any(|w| w == b"EXIF")),
        ImageFormat::Gif => (strip_gif(data)?, false),
        _ => return Ok(data.to_vec()),
    };
    let resize = max > 0 && crate::attach::dims(data).is_some_and(|(w, h)| w.max(h) > max);
    if !resize && !oriented {
        return Ok(clean);
    }
    // Keep animation intact. GIF frames can be resized; the WebP/APNG encoder cannot retain
    // animation, so refuse that resize rather than silently replacing it with one frame.
    if format == ImageFormat::Gif {
        let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(&clean))
            .map_err(|e| invalid(format!("can't prepare GIF: {e}")))?;
        decoder
            .set_limits(image::Limits::default())
            .map_err(|e| invalid(format!("GIF exceeds decoder limits: {e}")))?;
        let mut out = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
            // The original loop extension is retained below (finite counts included).
            if let Some(repeat) = gif_repeat(&clean) {
                encoder.set_repeat(repeat)?;
            }
            for frame in decoder.into_frames() {
                let frame = frame.map_err(|e| invalid(format!("can't prepare GIF: {e}")))?;
                let delay = frame.delay();
                let img = image::DynamicImage::ImageRgba8(frame.into_buffer()).resize(
                    max,
                    max,
                    image::imageops::FilterType::Triangle,
                );
                encoder.encode_frame(image::Frame::from_parts(img.into_rgba8(), 0, 0, delay))?;
            }
        }
        return Ok(out);
    }
    if (format == ImageFormat::WebP && clean.windows(4).any(|w| w == b"ANIM"))
        || (format == ImageFormat::Png && clean.windows(4).any(|w| w == b"acTL"))
    {
        if resize {
            return Err(invalid(
                "animated WebP/APNG exceeds attachments.max_dimension · increase the setting or set it to 0",
            ));
        }
        return Ok(clean);
    }
    let mut decoder = ImageReader::with_format(Cursor::new(data), format)
        .into_decoder()
        .map_err(|e| invalid(format!("can't prepare image: {e}")))?;
    let orientation = decoder
        .orientation()
        .map_err(|e| invalid(format!("can't read image orientation: {e}")))?;
    if !resize && orientation == image::metadata::Orientation::NoTransforms {
        return Ok(clean);
    }
    let mut img = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| invalid(format!("can't prepare image: {e}")))?;
    img.apply_orientation(orientation);
    if max > 0 && img.width().max(img.height()) > max {
        img = img.resize(max, max, image::imageops::FilterType::Triangle);
    }
    let mut out = Cursor::new(Vec::new());
    if format == ImageFormat::Jpeg {
        img.to_rgb8().write_to(&mut out, format)?;
    } else {
        img.write_to(&mut out, format)?;
    }
    Ok(out.into_inner())
}

fn strip_jpeg(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = data[..2].to_vec();
    let mut i = 2;
    while i < data.len() {
        let start = i;
        if data[i] != 0xff {
            return Err(invalid("invalid JPEG marker"));
        }
        while data.get(i) == Some(&0xff) {
            i += 1;
        }
        let marker = *data.get(i).ok_or_else(|| invalid("truncated JPEG"))?;
        i += 1;
        if marker == 0xda || marker == 0xd9 {
            out.extend_from_slice(&data[start..]);
            break;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            out.extend_from_slice(&data[start..i]);
            continue;
        }
        let len = data
            .get(i..i + 2)
            .ok_or_else(|| invalid("truncated JPEG"))?;
        let len = u16::from_be_bytes([len[0], len[1]]) as usize;
        if len < 2 || i + len > data.len() {
            return Err(invalid("invalid JPEG segment length"));
        }
        // APP1: EXIF/XMP, APP13: IPTC/Photoshop, COM: free-form camera/user comments.
        if !matches!(marker, 0xe1 | 0xed | 0xfe) {
            out.extend_from_slice(&data[start..i + len]);
        }
        i += len;
    }
    Ok(out)
}

fn strip_png(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = data[..8].to_vec();
    let mut i = 8;
    while i < data.len() {
        let head = data
            .get(i..i + 8)
            .ok_or_else(|| invalid("truncated PNG chunk"))?;
        let len = u32::from_be_bytes(head[..4].try_into().unwrap()) as usize;
        let end = i
            .checked_add(len)
            .and_then(|n| n.checked_add(12))
            .filter(|n| *n <= data.len())
            .ok_or_else(|| invalid("invalid PNG chunk length"))?;
        if !matches!(&head[4..8], b"eXIf" | b"tEXt" | b"zTXt" | b"iTXt") {
            out.extend_from_slice(&data[i..end]);
        }
        i = end;
    }
    Ok(out)
}

fn strip_webp(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = data[..12].to_vec();
    let mut i = 12;
    while i < data.len() {
        let head = data
            .get(i..i + 8)
            .ok_or_else(|| invalid("truncated WebP chunk"))?;
        let len = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
        let end = i
            .checked_add(8)
            .and_then(|n| n.checked_add(len))
            .and_then(|n| n.checked_add(len % 2))
            .filter(|n| *n <= data.len())
            .ok_or_else(|| invalid("invalid WebP chunk length"))?;
        if !matches!(&head[..4], b"EXIF" | b"XMP ") {
            let at = out.len();
            out.extend_from_slice(&data[i..end]);
            if &head[..4] == b"VP8X" && len > 0 {
                out[at + 8] &= !(0x08 | 0x04);
            }
        }
        i = end;
    }
    let len = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&len.to_le_bytes());
    Ok(out)
}

fn strip_gif(data: &[u8]) -> Result<Vec<u8>> {
    // Some legacy fixtures consist only of a dimensions header.
    if data.len() < 13 {
        return Ok(data.to_vec());
    }
    let mut i = 13
        + if data[10] & 0x80 != 0 {
            3 * (1usize << ((data[10] & 7) + 1))
        } else {
            0
        };
    if i > data.len() {
        return Err(invalid("truncated GIF palette"));
    }
    let mut out = data[..i].to_vec();
    while i < data.len() {
        let start = i;
        let keep = match data[i] {
            0x3b => {
                out.push(0x3b);
                break;
            }
            0x21 => {
                let label = *data
                    .get(i + 1)
                    .ok_or_else(|| invalid("truncated GIF extension"))?;
                i += 2;
                let keep = label == 0xf9
                    || (label == 0xff
                        && (data.get(i + 1..i + 12) == Some(b"NETSCAPE2.0")
                            || data.get(i + 1..i + 12) == Some(b"ANIMEXTS1.0")));
                gif_blocks(data, &mut i)?;
                keep
            }
            0x2c => {
                let desc = data
                    .get(i..i + 10)
                    .ok_or_else(|| invalid("truncated GIF frame"))?;
                i += 10
                    + if desc[9] & 0x80 != 0 {
                        3 * (1usize << ((desc[9] & 7) + 1))
                    } else {
                        0
                    };
                i += 1; // LZW code size.
                gif_blocks(data, &mut i)?;
                true
            }
            _ => return Err(invalid("invalid GIF block")),
        };
        if keep {
            out.extend_from_slice(&data[start..i]);
        }
    }
    Ok(out)
}

fn gif_blocks(data: &[u8], i: &mut usize) -> Result<()> {
    loop {
        let n = *data.get(*i).ok_or_else(|| invalid("truncated GIF data"))? as usize;
        *i += n + 1;
        if *i > data.len() {
            return Err(invalid("truncated GIF data"));
        }
        if n == 0 {
            return Ok(());
        }
    }
}

fn gif_repeat(data: &[u8]) -> Option<image::codecs::gif::Repeat> {
    for name in [b"NETSCAPE2.0", b"ANIMEXTS1.0"] {
        if let Some(i) = data.windows(11).position(|w| w == name) {
            let b = data.get(i + 11..i + 16)?;
            if b[0..2] == [3, 1] {
                let n = u16::from_le_bytes([b[2], b[3]]);
                return Some(if n == 0 {
                    image::codecs::gif::Repeat::Infinite
                } else {
                    image::codecs::gif::Repeat::Finite(n)
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(format: ImageFormat, w: u32, h: u32) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            w,
            h,
            image::Rgb([40, 90, 120]),
        ));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    fn jpeg_exif(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        exif.extend(orientation.to_le_bytes());
        exif.extend([0; 6]);
        exif.extend(b"GPS camera serial location");
        let mut out = jpeg[..2].to_vec();
        out.extend([0xff, 0xe1]);
        out.extend(((exif.len() + 2) as u16).to_be_bytes());
        out.extend(exif);
        out.extend(&jpeg[2..]);
        out
    }

    #[test]
    fn jpeg_private_metadata_is_losslessly_removed_and_orientation_is_preserved() {
        let jpeg = image(ImageFormat::Jpeg, 8, 4);
        assert_eq!(prepare(&jpeg_exif(&jpeg, 1), 4096).unwrap(), jpeg);
        let rotated = prepare(&jpeg_exif(&jpeg, 6), 0).unwrap();
        assert_eq!(crate::attach::dims(&rotated), Some((4, 8)));
        assert!(!rotated.windows(4).any(|w| w == b"Exif"));
        assert!(!rotated.windows(3).any(|w| w == b"GPS"));
    }

    #[test]
    fn still_images_resize_in_proportion_and_zero_disables_resizing() {
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
            let original = image(format, 16, 8);
            assert_eq!(prepare(&original, 0).unwrap(), original);
            let resized = prepare(&original, 4).unwrap();
            let decoded = image::load_from_memory(&resized).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (4, 2));
            assert_eq!(
                prepare(&resized, 4).unwrap(),
                resized,
                "stable stored bytes"
            );
        }
    }

    #[test]
    fn png_and_webp_private_chunks_are_removed_without_reencoding_pixels() {
        let png = image(ImageFormat::Png, 8, 4);
        // The stripped chunk's CRC doesn't matter: it is never stored or decoded.
        let mut with_text = png[..33].to_vec();
        let text = b"camera\0GPS location and serial number";
        with_text.extend((text.len() as u32).to_be_bytes());
        with_text.extend(b"tEXt");
        with_text.extend(text);
        with_text.extend([0; 4]);
        with_text.extend(&png[33..]);
        assert_eq!(prepare(&with_text, 4096).unwrap(), png);
        let webp = image(ImageFormat::WebP, 8, 4);
        let mut with_xmp = webp.clone();
        with_xmp.extend(b"XMP \x04\0\0\0GPS!");
        let len = (with_xmp.len() - 8) as u32;
        with_xmp[4..8].copy_from_slice(&len.to_le_bytes());
        assert_eq!(prepare(&with_xmp, 4096).unwrap(), webp);
    }

    #[test]
    fn animated_gif_keeps_frames_delays_and_finite_loop_count_when_resized() {
        let mut gif = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut gif);
            enc.set_repeat(image::codecs::gif::Repeat::Finite(3))
                .unwrap();
            for c in [20, 180] {
                enc.encode_frame(image::Frame::from_parts(
                    image::RgbaImage::from_pixel(8, 4, image::Rgba([c, 80, 40, 255])),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(50, 1),
                ))
                .unwrap();
            }
        }
        let mut with_comment = gif[..gif.len() - 1].to_vec();
        with_comment.extend(b"\x21\xfe\x03GPS\0\x3b");
        assert_eq!(prepare(&with_comment, 0).unwrap(), gif);
        let resized = prepare(&with_comment, 2).unwrap();
        let frames = image::codecs::gif::GifDecoder::new(Cursor::new(&resized))
            .unwrap()
            .into_frames()
            .collect_frames()
            .unwrap();
        assert_eq!(frames.len(), 2);
        for f in frames {
            assert_eq!(f.buffer().dimensions(), (2, 1));
            assert_eq!(f.delay().numer_denom_ms(), (50, 1));
        }
        assert!(matches!(
            gif_repeat(&resized),
            Some(image::codecs::gif::Repeat::Finite(3))
        ));
    }

    #[test]
    fn malformed_metadata_fails_instead_of_storing_private_bytes() {
        assert!(strip_png(b"\x89PNG\r\n\x1a\n\xff\xff\xff\xfftEXtGPS").is_err());
        assert!(strip_jpeg(b"\xff\xd8\xff\xe1\xff\xffExif\0\0GPS").is_err());
        assert!(strip_webp(b"RIFF\x10\0\0\0WEBPEXIF\xff\xff\xff\xffGPS").is_err());
    }
}
