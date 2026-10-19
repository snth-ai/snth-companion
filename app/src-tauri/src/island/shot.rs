//! The screenshot of "screenshot + voice".
//!
//! First choice: the app's own helper (the Cua driver it installs and
//! runs embedded, under the app's own Screen Recording permission), its
//! `get_desktop_state`, asked by the app itself through the runtime that
//! also reads the list of apps (never through the synth's pipe). macOS
//! falls back to the system's `screencapture` on the display the pointer
//! is on (the same permission: it runs as the app's child). Either way the
//! picture is scaled to at most 2560 px on its long side and sent as JPEG
//! at quality 80.
//!
//! The island is not in the picture: its window is excluded from screen
//! capture (`panel.rs`: `NSWindowSharingNone`, `WDA_EXCLUDEFROMCAPTURE`)
//! and also made invisible for the moment of the capture.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use base64::Engine;
use connect_core::island::{Image, MAX_IMAGE_EDGE};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use serde_json::Value;

pub const JPEG_QUALITY: u8 = 80;

/// Any picture (PNG or JPEG bytes) as the JPEG the synth gets.
pub fn to_jpeg(bytes: &[u8], max_edge: u32) -> Result<Image, String> {
    let picture = image::load_from_memory(bytes).map_err(|e| format!("shot_failed: not a picture: {e}"))?;
    let (w, h) = (picture.width(), picture.height());
    let long = w.max(h);
    let picture = if long > max_edge {
        let scale = f64::from(max_edge) / f64::from(long);
        let (nw, nh) = (((f64::from(w) * scale).round() as u32).max(1), ((f64::from(h) * scale).round() as u32).max(1));
        picture.resize_exact(nw, nh, FilterType::Triangle)
    } else {
        picture
    };
    let rgb = picture.to_rgb8();
    let mut out = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&rgb)
        .map_err(|e| format!("shot_failed: {e}"))?;
    Ok(Image { mime: "image/jpeg".into(), data: out.into_inner(), long_edge: rgb.width().max(rgb.height()) })
}

/// An image the helper put inline in its answer (MCP image content).
fn inline_picture(result: &Value) -> Option<Vec<u8>> {
    result.get("content")?.as_array()?.iter().find_map(|c| {
        if c.get("type").and_then(Value::as_str) != Some("image") {
            return None;
        }
        base64::engine::general_purpose::STANDARD.decode(c.get("data")?.as_str()?).ok()
    })
}

fn temp_file(data_dir: &Path, ext: &str) -> PathBuf {
    let dir = data_dir.join("island");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("shot-{}.{ext}", std::process::id()))
}

/// Take the picture. `display` is the display under the pointer, for the
/// fallback (macOS `screencapture -D`, 1-based).
pub async fn take(core: &connect_core::Core, data_dir: &Path, display: Option<u32>) -> Result<Image, String> {
    let out = temp_file(data_dir, "png");
    let _ = std::fs::remove_file(&out);
    let helper = core.desktop_picture(&out, MAX_IMAGE_EDGE).await;
    let bytes = match &helper {
        Ok(result) => std::fs::read(&out).ok().filter(|b| !b.is_empty()).or_else(|| inline_picture(result)),
        Err(_) => None,
    };
    let _ = std::fs::remove_file(&out);
    if let Some(bytes) = bytes {
        return tokio::task::spawn_blocking(move || to_jpeg(&bytes, MAX_IMAGE_EDGE)).await.map_err(|e| e.to_string())?;
    }
    let why = helper.err().unwrap_or_else(|| "the helper gave no picture".into());
    connect_core::logln!("island: screenshot through the helper: {why}");
    fallback(data_dir, display).await.map_err(|e| format!("{e} (helper: {why})"))
}

#[cfg(target_os = "macos")]
async fn fallback(data_dir: &Path, display: Option<u32>) -> Result<Image, String> {
    let out = temp_file(data_dir, "jpg");
    let _ = std::fs::remove_file(&out);
    let mut cmd = tokio::process::Command::new("/usr/sbin/screencapture");
    // -x: no sound; -t jpg; -D: the display (1 = main).
    cmd.args(["-x", "-t", "jpg"]);
    if let Some(d) = display {
        cmd.arg(format!("-D{d}"));
    }
    cmd.arg(&out);
    let status = tokio::time::timeout(std::time::Duration::from_secs(8), cmd.status())
        .await
        .map_err(|_| "shot_failed: screencapture did not finish".to_string())?
        .map_err(|e| format!("shot_failed: {e}"))?;
    let bytes = std::fs::read(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    // Without Screen Recording permission macOS hands back the desktop
    // picture only, or nothing.
    if !status.success() || bytes.is_empty() {
        return Err("shot_permission: no picture (Screen Recording is not allowed for SNTH Connect)".into());
    }
    tokio::task::spawn_blocking(move || to_jpeg(&bytes, MAX_IMAGE_EDGE)).await.map_err(|e| e.to_string())?
}

#[cfg(not(target_os = "macos"))]
async fn fallback(_data_dir: &Path, _display: Option<u32>) -> Result<Image, String> {
    Err("shot_failed: the helper could not take a picture".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_big_picture_is_scaled_to_the_limit_and_sent_as_jpeg() {
        let shot = to_jpeg(&png(3024, 1964), 2560).unwrap();
        assert_eq!(shot.mime, "image/jpeg");
        assert_eq!(shot.long_edge, 2560);
        let back = image::load_from_memory(&shot.data).unwrap();
        assert_eq!((back.width(), back.height()), (2560, 1663));
        assert_eq!(&shot.data[..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn a_small_picture_keeps_its_size() {
        let shot = to_jpeg(&png(800, 600), 2560).unwrap();
        assert_eq!(shot.long_edge, 800);
        assert!(to_jpeg(b"not a picture", 2560).is_err());
    }

    #[test]
    fn an_inline_picture_is_found() {
        let data = base64::engine::general_purpose::STANDARD.encode(png(4, 4));
        let result = serde_json::json!({"content": [{"type": "text", "text": "tree"}, {"type": "image", "data": data, "mimeType": "image/png"}]});
        assert!(inline_picture(&result).is_some());
        assert!(inline_picture(&serde_json::json!({"content": []})).is_none());
    }
}
