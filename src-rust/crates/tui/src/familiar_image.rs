// familiar_image.rs — Render familiar card images in image-capable terminals.
//
// Uses runtime file lookup so that images remain personal to each user and are
// never embedded in the binary.  Falls back gracefully to block-art glyphs when
// no image is found or the terminal does not support inline graphics.

use crate::kitty_image::{detect_image_protocol, ImageProtocol};
use base64::Engine;

const FAMILIAR_IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// Look up a user's familiar image at runtime from known paths.
///
/// Returns the first matching file path, or `None` if no image is found.
/// Never panics; always degrades gracefully.
pub fn familiar_image_path(familiar_id: &str) -> Option<std::path::PathBuf> {
    if !is_safe_familiar_id(familiar_id) {
        return None;
    }

    let search_dirs: Vec<std::path::PathBuf> = [
        dirs::home_dir().map(|h| h.join(".coven").join("assets").join("familiars")),
        dirs::home_dir().map(|h| h.join(".coven-code").join("assets").join("familiars")),
    ]
    .into_iter()
    .flatten()
    .collect();

    familiar_image_path_in_dirs(familiar_id, &search_dirs)
}

fn familiar_image_path_in_dirs(
    familiar_id: &str,
    search_dirs: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    if !is_safe_familiar_id(familiar_id) {
        return None;
    }

    for dir in search_dirs {
        for ext in &FAMILIAR_IMAGE_EXTENSIONS {
            let p = dir.join(format!("{}.{}", familiar_id, ext));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn is_safe_familiar_id(familiar_id: &str) -> bool {
    !familiar_id.is_empty()
        && familiar_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// Attempt to render the familiar card image as an inline terminal image sequence.
///
/// Returns `Some(escape_sequence_string)` when the terminal supports image
/// rendering (Kitty or Sixel protocol detected) and an image file is found at
/// runtime in `~/.coven/assets/familiars/` or `~/.coven-code/assets/familiars/`.
/// Returns `None` when no image protocol is available or no file is found,
/// so the caller can fall back to block-art glyphs.
///
/// `width_cells` and `height_cells` are passed as Kitty `c=`/`r=` column/row
/// counts when non-zero. Sixel rendering currently uses the source image size.
pub fn render_familiar_image(
    familiar_id: &str,
    width_cells: u16,
    height_cells: u16,
) -> Option<String> {
    let protocol = detect_image_protocol();
    if protocol == ImageProtocol::Text {
        return None;
    }

    let path = familiar_image_path(familiar_id)?;
    let bytes = std::fs::read(&path).ok()?;

    match protocol {
        ImageProtocol::Kitty => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Some(build_kitty_sequence(&b64, width_cells, height_cells))
        }
        ImageProtocol::Sixel => build_sixel_sequence(&bytes),
        ImageProtocol::Text => None,
    }
}

// ---------------------------------------------------------------------------
// Kitty inline sequence builder (returns String instead of writing to stdout)
// ---------------------------------------------------------------------------

/// Build a Kitty APC escape sequence string for a base64-encoded image payload.
///
/// We return the full sequence as a `String` so ratatui can embed it in a Span
/// without writing directly to stdout, which would race with ratatui's own
/// rendering pipeline.
fn build_kitty_sequence(base64_data: &str, width_cells: u16, height_cells: u16) -> String {
    const CHUNK: usize = 4096;

    let clean: String = base64_data.chars().filter(|c| !c.is_whitespace()).collect();

    let raw_len = clean.len();
    let total_chunks = (raw_len + CHUNK - 1).max(1) / CHUNK;

    let mut out = String::with_capacity(raw_len + total_chunks * 32);

    let mut offset = 0;
    let mut first = true;
    while offset < raw_len {
        let end = (offset + CHUNK).min(raw_len);
        let chunk = &clean[offset..end];
        let more: u8 = if end < raw_len { 1 } else { 0 };

        let params = if first {
            let mut params = format!("a=T,f=100,m={},q=2,C=1", more);
            if width_cells > 0 {
                params.push_str(&format!(",c={}", width_cells));
            }
            if height_cells > 0 {
                params.push_str(&format!(",r={}", height_cells));
            }
            params
        } else {
            format!("a=T,m={},q=2", more)
        };

        out.push_str("\x1b_G");
        out.push_str(&params);
        out.push(';');
        out.push_str(chunk);
        out.push_str("\x1b\\");

        first = false;
        offset = end;
    }

    out
}

// ---------------------------------------------------------------------------
// Sixel sequence builder
// ---------------------------------------------------------------------------

/// Convert raw image bytes to a Sixel escape sequence string.
///
/// Returns `None` if any step of decoding or conversion fails.
fn build_sixel_sequence(image_bytes: &[u8]) -> Option<String> {
    use icy_sixel::encoder::EncodeOptions;
    use image::ImageReader;
    use std::io::Cursor;

    // Decode image
    let reader = ImageReader::new(Cursor::new(image_bytes))
        .with_guessed_format()
        .ok()?;
    let img = reader.decode().ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let pixels = rgba.into_raw();

    // Convert to Sixel
    let sixel_str = icy_sixel::encoder::sixel_encode(
        &pixels,
        width as usize,
        height as usize,
        &EncodeOptions::default(),
    )
    .ok()?;

    // Wrap with Sixel DCS delimiters
    let mut out = String::with_capacity(sixel_str.len() + 8);
    out.push_str("\x1bPq");
    out.push_str(&sixel_str);
    out.push_str("\x1b\\");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_familiar_dir(test_name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("coven-familiar-image-{test_name}-{unique}"));
        fs::create_dir_all(&dir).expect("test familiar dir should be created");
        dir
    }

    #[test]
    fn familiar_image_lookup_finds_first_known_extension() {
        let dir = temp_familiar_dir("lookup");
        let image_path = dir.join("cody.webp");
        fs::write(&image_path, b"not actually decoded in lookup").expect("test image should write");

        let found = familiar_image_path_in_dirs("cody", &[dir.clone()]);

        assert_eq!(found.as_deref(), Some(image_path.as_path()));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn familiar_image_lookup_rejects_path_like_ids() {
        let dir = temp_familiar_dir("path-like");
        fs::create_dir_all(dir.join("..")).ok();

        assert!(familiar_image_path_in_dirs("../cody", &[dir.clone()]).is_none());
        assert!(familiar_image_path_in_dirs("cody/image", &[dir.clone()]).is_none());
        assert!(familiar_image_path_in_dirs("", &[dir.clone()]).is_none());

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn kitty_sequence_wraps_payload_with_size_hints() {
        let seq = build_kitty_sequence("YW Jj\nZA==", 11, 5);

        assert!(seq.starts_with("\x1b_Ga=T,f=100,m=0,q=2,C=1,c=11,r=5;"));
        assert!(seq.contains("YWJjZA=="));
        assert!(seq.ends_with("\x1b\\"));
    }

    #[test]
    fn kitty_sequence_omits_zero_size_hints() {
        let seq = build_kitty_sequence("abcd", 0, 0);

        assert!(seq.starts_with("\x1b_Ga=T,f=100,m=0,q=2,C=1;"));
        assert!(!seq.contains(",c="));
        assert!(!seq.contains(",r="));
    }
}
