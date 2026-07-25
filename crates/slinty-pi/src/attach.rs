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
}
