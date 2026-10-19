//! Thumbnails and zip downloads.
//!
//! Thumbnails are JPEGs cached in `~/.cache/holodrive/thumbs`, keyed by
//! path + size + mtime, so an edited picture gets a fresh one. Pictures go
//! through the `image` crate; videos (and anything `image` can't decode)
//! through `ffmpeg` when it's installed. At most two are made at once.

use crate::config::cache_dir;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Read the standard EXIF capture timestamp from the header of a JPEG.
/// The value is kept as camera-local text because EXIF usually has no timezone.
pub fn exif_capture_date(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(1024 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.get(..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut pos = 2usize;
    while pos + 4 <= bytes.len() {
        if bytes[pos] != 0xff {
            pos += 1;
            continue;
        }
        while bytes.get(pos) == Some(&0xff) {
            pos += 1;
        }
        let marker = *bytes.get(pos)?;
        pos += 1;
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let length = u16::from_be_bytes([*bytes.get(pos)?, *bytes.get(pos + 1)?]) as usize;
        if length < 2 || pos + length > bytes.len() {
            break;
        }
        let segment = &bytes[pos + 2..pos + length];
        if marker == 0xe1
            && segment.starts_with(b"Exif\0\0")
            && let Some(value) = tiff_capture_date(&segment[6..])
        {
            return Some(value);
        }
        pos += length;
    }
    None
}

fn tiff_capture_date(bytes: &[u8]) -> Option<String> {
    let little = match bytes.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let value: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(value)
        } else {
            u16::from_be_bytes(value)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let value: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(value)
        } else {
            u32::from_be_bytes(value)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd0 = u32_at(4)? as usize;
    let count = u16_at(ifd0)? as usize;
    let mut exif_ifd = None;
    let mut fallback = None;
    for index in 0..count.min(512) {
        let entry = ifd0.checked_add(2)?.checked_add(index.checked_mul(12)?)?;
        let tag = u16_at(entry)?;
        let kind = u16_at(entry + 2)?;
        let amount = u32_at(entry + 4)? as usize;
        if tag == 0x8769 && kind == 4 && amount == 1 {
            exif_ifd = Some(u32_at(entry + 8)? as usize);
        } else if tag == 0x0132 {
            fallback = read_ascii_tag(bytes, entry, kind, amount, &u32_at);
        }
    }
    if let Some(ifd) = exif_ifd {
        let count = u16_at(ifd)? as usize;
        for index in 0..count.min(512) {
            let entry = ifd.checked_add(2)?.checked_add(index.checked_mul(12)?)?;
            let tag = u16_at(entry)?;
            if (tag == 0x9003 || tag == 0x9004)
                && let Some(value) = read_ascii_tag(
                    bytes,
                    entry,
                    u16_at(entry + 2)?,
                    u32_at(entry + 4)? as usize,
                    &u32_at,
                )
                && valid_exif_date(&value)
            {
                return Some(value);
            }
        }
    }
    fallback.filter(|value| valid_exif_date(value))
}

fn read_ascii_tag(
    bytes: &[u8],
    entry: usize,
    kind: u16,
    count: usize,
    u32_at: &impl Fn(usize) -> Option<u32>,
) -> Option<String> {
    if kind != 2 || count == 0 || count > 64 {
        return None;
    }
    let offset = if count <= 4 {
        entry.checked_add(8)?
    } else {
        u32_at(entry + 8)? as usize
    };
    let raw = bytes.get(offset..offset.checked_add(count)?)?;
    let value = String::from_utf8_lossy(raw)
        .trim_matches(char::from(0))
        .trim()
        .to_owned();
    Some(value)
}

fn valid_exif_date(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 19
        && [4, 7, 10, 13, 16]
            .iter()
            .all(|i| matches!(b[*i], b':' | b' '))
        && b.iter()
            .enumerate()
            .all(|(i, c)| [4, 7, 10, 13, 16].contains(&i) || c.is_ascii_digit())
}

pub const THUMB_PX: u32 = 360;

fn key(path: &Path, meta: &std::fs::Metadata) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok().hash(&mut h);
    THUMB_PX.hash(&mut h);
    format!("{:016x}", h.finish())
}

pub fn is_image(ext: &str) -> bool {
    matches!(
        ext,
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff"
    )
}

pub fn is_video(ext: &str) -> bool {
    matches!(ext, "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v")
}

/// Returns the cached JPEG path, making it if needed. `None` = no thumbnail
/// possible for this file.
pub fn thumbnail(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    let dir = cache_dir().join("thumbs");
    std::fs::create_dir_all(&dir).ok()?;
    let out = dir.join(format!("{}.jpg", key(path, &meta)));
    if out.exists() {
        return Some(out);
    }
    let tmp = dir.join(format!(".{}.tmp.jpg", crate::auth::random_hex(6)));
    let ok = if is_image(&ext) {
        image_thumb(path, &tmp).is_ok() || ffmpeg_thumb(path, &tmp, false)
    } else if is_video(&ext) {
        ffmpeg_thumb(path, &tmp, true)
    } else {
        false
    };
    if ok && std::fs::rename(&tmp, &out).is_ok() {
        Some(out)
    } else {
        let _ = std::fs::remove_file(&tmp);
        None
    }
}

fn image_thumb(src: &Path, dst: &Path) -> anyhow::Result<()> {
    let img = image::ImageReader::open(src)?
        .with_guessed_format()?
        .decode()?;
    let small = img.thumbnail(THUMB_PX, THUMB_PX).into_rgb8();
    let mut f = std::io::BufWriter::new(std::fs::File::create(dst)?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut f, 82).encode_image(&small)?;
    Ok(())
}

fn ffmpeg_thumb(src: &Path, dst: &Path, seek: bool) -> bool {
    let scale = format!("scale='min({THUMB_PX},iw)':-2");
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-loglevel", "error", "-y"]);
    if seek {
        cmd.args(["-ss", "2"]);
    }
    cmd.arg("-i")
        .arg(src)
        .args(["-frames:v", "1", "-vf", &scale, "-q:v", "4"])
        .arg(dst);
    cmd.status().map(|s| s.success()).unwrap_or(false) && dst.exists()
}

/// Zip `items` (absolute paths) into a temp file and return it. Hidden
/// files are left out, like everywhere else; symlinks are skipped.
pub fn zip_items(items: &[PathBuf]) -> anyhow::Result<PathBuf> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let dir = cache_dir().join("tmp");
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{}.zip", crate::auth::random_hex(8)));
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(std::fs::File::create(&out)?));
    let stored = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    let deflated = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    for item in items {
        let base = item.parent().unwrap_or(item);
        let walker = walkdir::WalkDir::new(item)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
        for e in walker.flatten() {
            let rel = e.path().strip_prefix(base)?.to_string_lossy().into_owned();
            if e.file_type().is_dir() {
                zip.add_directory(format!("{rel}/"), deflated)?;
            } else if e.file_type().is_file() {
                let ext = e
                    .path()
                    .extension()
                    .map(|x| x.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let already_small = is_image(&ext)
                    || is_video(&ext)
                    || matches!(
                        ext.as_str(),
                        "zip" | "gz" | "xz" | "zst" | "7z" | "mp3" | "flac" | "iso" | "pdf"
                    );
                zip.start_file(rel, if already_small { stored } else { deflated })?;
                let mut f = std::fs::File::open(e.path())?;
                std::io::copy(&mut f, &mut zip)?;
            }
        }
    }
    zip.finish()?.flush()?;
    Ok(out)
}

/// Startup housekeeping: forget zips left behind by a crash.
pub fn clean_tmp() {
    if let Ok(rd) = std::fs::read_dir(cache_dir().join("tmp")) {
        for e in rd.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod exif_tests {
    use super::exif_capture_date;

    #[test]
    fn reads_jpeg_exif_original_capture_time() {
        let mut tiff = b"II\x2a\0\x08\0\0\0\x01\0".to_vec();
        tiff.extend_from_slice(&[0x69, 0x87, 4, 0, 1, 0, 0, 0, 26, 0, 0, 0]);
        tiff.extend_from_slice(&[0; 4]);
        tiff.extend_from_slice(&[1, 0, 0x03, 0x90, 2, 0, 20, 0, 0, 0, 44, 0, 0, 0]);
        tiff.extend_from_slice(&[0; 4]);
        tiff.extend_from_slice(b"2024:02:03 04:05:06\0");
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        let segment_len = (tiff.len() + 8) as u16;
        jpeg.extend_from_slice(&segment_len.to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&tiff);
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        let path =
            std::env::temp_dir().join(format!("holodrive-exif-{}.jpg", crate::auth::random_hex(4)));
        std::fs::write(&path, jpeg).unwrap();
        assert_eq!(
            exif_capture_date(&path).as_deref(),
            Some("2024:02:03 04:05:06")
        );
        std::fs::remove_file(path).unwrap();
    }
}
