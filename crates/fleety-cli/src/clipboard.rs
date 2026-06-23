//! Cross-platform clipboard helpers for the TUI's Ctrl+V handling.
//!
//! Three cases, decided in this order:
//!
//! 1. **Image bytes** — the OS clipboard holds raw RGBA pixels (e.g. a
//!    screenshot, a copy from a paint program). We re-encode them as PNG and
//!    return them as an inline `image/png` [`WireAttachment`].
//! 2. **File path text** — the clipboard text is a single line that
//!    canonicalises to an existing file (common with file-manager drag-drop or
//!    "copy path"). We read the file, guess its MIME, and attach it.
//! 3. **Plain text** — anything else, returned as a string to insert into the
//!    input box.

use std::path::Path;

use agent_core::{CoreError, Result};
use base64::Engine;
use fleety_protocol::WireAttachment;

/// What we found on the clipboard.
pub enum ClipboardPaste {
    Image(WireAttachment),
    File(WireAttachment),
    Text(String),
    Empty,
}

/// Inspect the OS clipboard and decide what to do (see module docs for order).
/// Soft-fails: any clipboard backend error returns `Empty` with a tracing line
/// rather than crashing the TUI.
pub fn read() -> ClipboardPaste {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "clipboard unavailable");
            return ClipboardPaste::Empty;
        }
    };

    if let Ok(image) = clipboard.get_image() {
        match rgba_to_png(image.width as u32, image.height as u32, &image.bytes) {
            Ok(png) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
                return ClipboardPaste::Image(WireAttachment {
                    mime: "image/png".to_string(),
                    bytes_b64: Some(b64),
                    url: None,
                    name: Some("clipboard.png".to_string()),
                });
            }
            Err(e) => {
                tracing::warn!(report = ?e.report(), "could not encode clipboard image; falling through to text");
            }
        }
    }

    let text = match clipboard.get_text() {
        Ok(t) => t,
        Err(_) => return ClipboardPaste::Empty,
    };
    if text.is_empty() {
        return ClipboardPaste::Empty;
    }

    // Drag-drop on most desktops translates to "the file path as text". If
    // that's the case, attach the file directly instead of pasting the path.
    if let Some(att) = try_attach_as_file(&text) {
        return ClipboardPaste::File(att);
    }

    ClipboardPaste::Text(text)
}

/// If `text` is a single-line path to an existing file, read it and turn it
/// into an attachment with a best-guess MIME.
fn try_attach_as_file(text: &str) -> Option<WireAttachment> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.lines().count() != 1 {
        return None;
    }
    // Strip surrounding quotes that some shells / file managers add.
    let unquoted = trimmed
        .trim_start_matches('"')
        .trim_end_matches('"')
        .trim_start_matches('\'')
        .trim_end_matches('\'');
    let path = Path::new(unquoted);
    if !path.is_file() {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let mime = guess_mime_from_path(path);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(String::from)
        .unwrap_or_else(|| "clipboard-file".to_string());
    Some(WireAttachment {
        mime,
        bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(&bytes)),
        url: None,
        name: Some(name),
    })
}

fn guess_mime_from_path(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "pdf" => "application/pdf",
        "txt" | "md" | "rs" | "py" | "js" | "ts" | "go" | "html" | "css" => "text/plain",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// RGBA8 → PNG bytes via the `png` crate (no extra image-processing deps).
fn rgba_to_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|p| p.checked_mul(4))
        .ok_or_else(|| CoreError::Message("clipboard image dimensions overflow".to_string()))?;
    if rgba.len() != expected {
        return Err(CoreError::Message(format!(
            "clipboard image size mismatch ({} bytes, expected {})",
            rgba.len(),
            expected
        )));
    }
    let mut out = Vec::with_capacity(rgba.len() / 4);
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| CoreError::Message(format!("png header: {e}")))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| CoreError::Message(format!("png write: {e}")))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_to_png_encodes_a_small_image() {
        // 2x1 image: opaque red, opaque green.
        let rgba: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let png = rgba_to_png(2, 1, &rgba).expect("encode");
        // PNG signature is 8 bytes — easy fingerprint.
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn rgba_to_png_rejects_size_mismatch() {
        let rgba = vec![0u8; 7]; // not divisible by 4 for any dim
        assert!(rgba_to_png(2, 1, &rgba).is_err());
    }

    #[test]
    fn try_attach_as_file_returns_none_for_non_paths() {
        assert!(try_attach_as_file("just some text").is_none());
        assert!(try_attach_as_file("line one\nline two").is_none());
        assert!(try_attach_as_file("").is_none());
    }

    #[test]
    fn try_attach_as_file_reads_a_real_file() {
        let dir = std::env::temp_dir().join(format!("fleety-clip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk");
        let path = dir.join("hello.txt");
        std::fs::write(&path, b"hi clipboard").expect("write");
        let path_str = path.to_string_lossy().to_string();
        let att = try_attach_as_file(&path_str).expect("found");
        assert_eq!(att.mime, "text/plain");
        assert_eq!(att.name.as_deref(), Some("hello.txt"));
        assert!(att.bytes_b64.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn guess_mime_from_path_routes_common_types() {
        assert_eq!(guess_mime_from_path(Path::new("a.png")), "image/png");
        assert_eq!(guess_mime_from_path(Path::new("a.MP4")), "video/mp4");
        assert_eq!(
            guess_mime_from_path(Path::new("a.unknown")),
            "application/octet-stream"
        );
    }
}
