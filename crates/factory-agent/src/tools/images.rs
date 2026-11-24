//! `read_image`: puts an image file into the conversation as an image the
//! model sees, at that point. One too big for what models take (long edge
//! or bytes) is scaled down here, so the agent never resizes anything
//! itself; within bounds, the file's own bytes go as they are. Whether the
//! agent's model reads images at all is judged where the model is known
//! (`Agent::prepare`), not here.

use std::io::Cursor;
use std::path::Path;

use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::errors::Error;
use crate::tools::fs::workspace_path;
use crate::types::{MAX_IMAGE_BYTES, ToolResult, ToolResultPart};

/// The longest edge sent to a model: providers scale anything longer down
/// on their side anyway, so more pixels would only cost bytes and tokens.
const LONG_EDGE: u32 = 2048;
/// A file bigger than this isn't read at all.
const MAX_READ: u64 = 100 * 1024 * 1024;
/// An image with more pixels than this isn't decoded (memory).
const MAX_PIXELS: u64 = 100_000_000;

#[derive(Deserialize)]
struct ImageArgs {
    path: String,
}

/// The image type of `bytes`, by what they begin with, never the file's name.
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

fn refuse(message: String) -> Error {
    Error::Other(message)
}

pub async fn read_image(cwd: &Path, args: &Value, cancel: &CancellationToken) -> Result<ToolResult, Error> {
    let args: ImageArgs = serde_json::from_value(args.clone())?;
    let target = workspace_path(cwd, &args.path).await?;
    let size = tokio::fs::metadata(&target).await.map_err(|error| Error::fs(error, "stat", &target))?.len();
    if size > MAX_READ {
        return Err(refuse(format!("{} is {} MB; read_image reads images up to {} MB.", args.path, size >> 20, MAX_READ >> 20)));
    }
    let bytes = tokio::fs::read(&target).await.map_err(|error| Error::fs(error, "open", &target))?;
    if cancel.is_cancelled() {
        return Err(Error::Aborted);
    }
    let Some(mime) = sniff(&bytes) else {
        return Err(refuse(format!("{} isn't an image models read (PNG, JPEG, GIF or WebP). For text, use read_file.", args.path)));
    };
    let (width, height) = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|error| refuse(format!("{} can't be read: {error}", args.path)))?
        .into_dimensions()
        .map_err(|error| refuse(format!("{} doesn't hold a readable image: {error}", args.path)))?;
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(refuse(format!("{} is {width}×{height}, too many pixels to read.", args.path)));
    }
    let fits = width.max(height) <= LONG_EDGE && bytes.len() as u64 <= MAX_IMAGE_BYTES;
    let (out, out_mime, shown) = if fits {
        (bytes, mime, None)
    } else {
        let scaled = tokio::task::spawn_blocking(move || fit(&bytes, mime)).await.map_err(std::io::Error::other)?;
        let fitted = scaled.map_err(|error| refuse(format!("{} can't be read: {error}", args.path)))?;
        (fitted.bytes, fitted.mime, Some((fitted.width, fitted.height)))
    };
    if cancel.is_cancelled() {
        return Err(Error::Aborted);
    }
    let name = |mime: &str| mime.trim_start_matches("image/").to_uppercase();
    let note = match shown {
        None => format!("{} ({width}×{height} {}).", args.path, name(mime)),
        Some((w, h)) => {
            format!(
                "{} ({width}×{height} {}), scaled down to {w}×{h} {} to fit what your model takes.",
                args.path,
                name(mime),
                name(out_mime)
            )
        }
    };
    let details = json!({"path": args.path, "width": width, "height": height, "scaled": shown.is_some()});
    Ok(ToolResult {
        content: vec![
            ToolResultPart::Text { text: note },
            ToolResultPart::Image {
                data: base64::engine::general_purpose::STANDARD.encode(&out),
                mime_type: out_mime.to_owned(),
                digest: Some(digest(&out)),
            },
        ],
        details,
        usage: None,
        terminate: None,
    })
}

/// An image's bytes as a tool result shows them to the model, as read_image
/// sends a file (#403: an image an agent made): as they are when they fit
/// what models take, else scaled down; with the size it shows at when
/// scaled. `width` and `height` are the image's own.
pub async fn model_part(bytes: Vec<u8>, width: u32, height: u32) -> Result<(ToolResultPart, Option<(u32, u32)>), String> {
    let Some(mime) = sniff(&bytes) else { return Err("it isn't an image models read (PNG, JPEG, GIF or WebP)".to_owned()) };
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("it's {width}×{height}, too many pixels to read"));
    }
    let fits = width.max(height) <= LONG_EDGE && bytes.len() as u64 <= MAX_IMAGE_BYTES;
    let (out, out_mime, shown) = if fits {
        (bytes, mime, None)
    } else {
        let fitted = tokio::task::spawn_blocking(move || fit(&bytes, mime)).await.map_err(|error| error.to_string())??;
        (fitted.bytes, fitted.mime, Some((fitted.width, fitted.height)))
    };
    let part = ToolResultPart::Image {
        data: base64::engine::general_purpose::STANDARD.encode(&out),
        mime_type: out_mime.to_owned(),
        digest: Some(digest(&out)),
    };
    Ok((part, shown))
}

/// The SHA-256 of an image's bytes, in hex: how a provider that keeps
/// images as files (Anthropic's, #382) knows it has sent this one.
pub fn digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What `fit` made: the bytes to send, their type and the size they show.
struct Fitted {
    bytes: Vec<u8>,
    mime: &'static str,
    width: u32,
    height: u32,
}

/// Scales an image down to `LONG_EDGE` and under `MAX_IMAGE_BYTES`: PNG out
/// (JPEG stays JPEG), falling back to JPEG and then to smaller sizes when
/// the bytes still don't fit.
fn fit(bytes: &[u8], mime: &'static str) -> Result<Fitted, String> {
    let decoded = image::load_from_memory(bytes).map_err(|error| error.to_string())?;
    let mut img = if decoded.width().max(decoded.height()) > LONG_EDGE {
        decoded.resize(LONG_EDGE, LONG_EDGE, image::imageops::FilterType::CatmullRom)
    } else {
        decoded
    };
    let png = mime != "image/jpeg";
    if png {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).map_err(|error| error.to_string())?;
        if out.len() as u64 <= MAX_IMAGE_BYTES {
            return Ok(Fitted { bytes: out, mime: "image/png", width: img.width(), height: img.height() });
        }
    }
    // A photo, or pixels PNG can't make small enough: JPEG, smaller if need be.
    for quality in [85_u8, 75, 60] {
        let mut out = Cursor::new(Vec::new());
        let rgb = image::DynamicImage::from(img.to_rgb8());
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
        rgb.write_with_encoder(encoder).map_err(|error| error.to_string())?;
        let out = out.into_inner();
        if out.len() as u64 <= MAX_IMAGE_BYTES {
            return Ok(Fitted { bytes: out, mime: "image/jpeg", width: img.width(), height: img.height() });
        }
        img = img.resize(img.width().div_ceil(2).max(1), img.height().div_ceil(2).max(1), image::imageops::FilterType::CatmullRom);
    }
    Err("it couldn't be made small enough".to_owned())
}
