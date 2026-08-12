//! Attach files/images to prompts (SW9). Ports (not shares — this crate
//! doesn't depend on `pi-core`, matching its established posture)
//! `pi_core::backend`'s `attach_path`, plus `pi_core::attach`'s
//! `image_mime_type`/`encode_base64` verbatim — the Slint app's reference
//! implementation already proves this pattern end-to-end (including its own
//! `CustomApplicationHandler` workaround for Finder drag-and-drop, which
//! SwiftUI's native `.dropDestination` makes unnecessary here), so this
//! crate needs only the FFI-facing wiring around it.

use std::path::Path;

use base64::Engine;
use pi_rpc::ImageContent;

use crate::{report_error, ChatSink};

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

/// Classifies `path`: a non-image pushes an `@path` reference via
/// `ChatSink::on_composer_append` for Swift to splice into the composer
/// text; an image is read + base64-encoded and queued into `pending_images`,
/// with the running list of display names pushed via `ChatSink::
/// on_pending_attachments_changed` (the chip row). Ports `pi_core::backend::
/// attach_path`.
pub async fn attach_path(
    pending_images: &mut Vec<(String, ImageContent)>,
    path: &Path,
    sink: &dyn ChatSink,
) {
    let Some(mime_type) = image_mime_type(path) else {
        sink.on_composer_append(path.display().to_string());
        return;
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let data = encode_base64(&bytes);
            pending_images.push((
                name,
                ImageContent {
                    kind: "image".to_string(),
                    data,
                    mime_type: mime_type.to_string(),
                },
            ));
            sink.on_pending_attachments_changed(
                pending_images.iter().map(|(n, _)| n.clone()).collect(),
            );
        }
        Err(e) => report_error(sink, format!("could not read {}: {e}", path.display())),
    }
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
