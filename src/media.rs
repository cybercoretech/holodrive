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
