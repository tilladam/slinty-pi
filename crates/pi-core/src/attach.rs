//! Composer attachments: classify a picked/dropped path as an inline image
//! or a plain `@path` reference, and read+base64-encode images for pi's
//! `ImageContent` prompt field.
//!
//! Slint 1.17's `DropArea`/`DragArea` are intra-UI drag primitives (see
//! `DropEvent { mime_type, data, position }` in slint-core) — winit's
//! `WindowEvent::DroppedFile` isn't consumed anywhere in Slint's winit
//! backend, so a real Finder-drag-into-window has no path through Slint's
//! own event handling. `main.rs` works around this by installing an
//! `i_slint_backend_winit::CustomApplicationHandler`, which sees winit's
//! `WindowEvent`s before Slint's event loop does and forwards
//! `DroppedFile` paths in as `UiCmd::AttachPath` — the same command the
//! attach button already sends per picked file.

use std::path::Path;

use base64::Engine;
use pi_rpc::ImageContent;

use crate::backend::UiSink;

/// Maps a handful of common image extensions to their MIME type. `None`
/// means the composer should treat `path` as a plain `@path` reference
/// instead of an inline image.
pub fn image_mime_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => return None,
    })
}

pub fn encode_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Pushes an already-encoded image into `pending_images` and notifies the
/// chip row — the shared tail end of both `attach_path`'s image branch
/// (bytes read from disk) and pasted image data (bytes with no path at
/// all, encoded client-side via `encode_png`).
pub fn queue_image(
    pending_images: &mut Vec<(String, ImageContent)>,
    name: String,
    mime_type: String,
    data: String,
    ui: &dyn UiSink,
) {
    pending_images.push((
        name,
        ImageContent {
            kind: "image".to_string(),
            data,
            mime_type,
        },
    ));
    ui.set_pending_attachments(pending_images.iter().map(|(n, _)| n.clone()).collect());
}

/// Encodes a raw RGBA8 buffer (as read from the clipboard via
/// `arboard::Clipboard::get_image`) into PNG bytes, since `ImageContent`
/// needs an actual encoded image format, not raw pixels.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn recognizes_common_image_extensions_case_insensitively() {
        for (name, mime) in [
            ("photo.png", "image/png"),
            ("photo.PNG", "image/png"),
            ("photo.jpg", "image/jpeg"),
            ("photo.jpeg", "image/jpeg"),
            ("photo.gif", "image/gif"),
            ("photo.webp", "image/webp"),
            ("photo.bmp", "image/bmp"),
        ] {
            assert_eq!(image_mime_type(&PathBuf::from(name)), Some(mime), "{name}");
        }
    }

    #[test]
    fn non_image_extensions_and_extensionless_paths_are_not_images() {
        assert_eq!(image_mime_type(&PathBuf::from("notes.txt")), None);
        assert_eq!(image_mime_type(&PathBuf::from("src/main.rs")), None);
        assert_eq!(image_mime_type(&PathBuf::from("README")), None);
    }

    #[test]
    fn encode_base64_round_trips() {
        let encoded = encode_base64(b"hello");
        assert_eq!(encoded, "aGVsbG8=");
    }

    #[test]
    fn encode_png_produces_valid_png_bytes() {
        // A single opaque red pixel.
        let rgba = [255u8, 0, 0, 255];
        let png_bytes = encode_png(1, 1, &rgba).expect("encode");
        assert_eq!(&png_bytes[..8], b"\x89PNG\r\n\x1a\n");
    }
}
