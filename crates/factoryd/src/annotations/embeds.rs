//! The pictures and videos a Markdown file shows in place (#540), as the
//! app decides them (app/lib/viewers/markdown_media.dart, Formaggio 43704):
//! `![alt](src)` in a paragraph or a list item, whose src is a local file
//! with a picture's or a video's extension, or a `data:` picture. The app
//! shows each as a block of its own, never its alt text; anything else
//! (a remote image, another kind of file, an image in a heading or a table
//! cell) shows its alt text as words.
//!
//! Two uses: the formatted view's plain text (`anchor::Projection`) leaves
//! them out, so a comment never anchors in an alt text nobody sees (row
//! 1449); and the files shown in panes are watched with their pictures, so
//! a picture that changes while its Markdown file doesn't is read again
//! (FileChanged for the picture's path, which the app's picture viewer
//! follows).

use base64::Engine as _;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// What the app reads as pictures, by extension.
const PICTURES: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
/// What it shows as a video's card.
const VIDEOS: [&str; 3] = ["mp4", "mov", "m4v"];
/// The most a `data:` picture may hold, decoded.
const DATA_MOST: usize = 5 * 1024 * 1024;

/// The parser's options, the same for every use of a Markdown file.
pub(crate) fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Media {
    Picture,
    Video,
}

/// Whether `path` is a picture or a video shown in place, by its extension.
pub(crate) fn media_kind(path: &str) -> Option<Media> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let (_, extension) = name.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    if PICTURES.contains(&extension.as_str()) {
        Some(Media::Picture)
    } else if VIDEOS.contains(&extension.as_str()) {
        Some(Media::Video)
    } else {
        None
    }
}

/// The file `src` names in the Markdown file at `file`, as the app's
/// `markdownPath` has it: beside it (its folder, then `.`, `..` and the
/// rest; none past the root), absolute, from the home folder (`~`, kept
/// as `~/…`), or a `file:` URL's path; a query or fragment is no part of a
/// plain name, which is percent-decoded. None for any other scheme.
pub(crate) fn markdown_path(file: &str, src: &str) -> Option<String> {
    let mut target = src.trim().to_owned();
    if target.is_empty() || target.starts_with('#') {
        return None;
    }
    match scheme(&target) {
        Some(scheme) if scheme.eq_ignore_ascii_case("file") => {
            let rest = &target[scheme.len() + 1..];
            // file:///path, file://host/path or file:/path: the path.
            let path = match rest.strip_prefix("//") {
                Some(authority) => authority.find('/').map_or("", |at| &authority[at..]),
                None => rest,
            };
            let path = path.split(['?', '#']).next().unwrap_or("");
            if path.is_empty() {
                return None;
            }
            target = path.to_owned();
        }
        Some(_) => return None,
        None => {
            if let Some(cut) = target.find(['?', '#']) {
                target.truncate(cut);
            }
        }
    }
    if let Some(decoded) = percent_decode(&target) {
        target = decoded;
    }
    if target.is_empty() {
        return None;
    }
    let joined = if target.starts_with('/') || target == "~" || target.starts_with("~/") {
        target
    } else {
        let folder = file.rfind('/').map_or("", |slash| &file[..slash]);
        format!("{folder}/{target}")
    };
    let home = joined.starts_with('~');
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/').skip(usize::from(home)) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!("{}/{}", if home { "~" } else { "" }, parts.join("/")))
}

/// A URL's scheme, as `[a-zA-Z][a-zA-Z0-9+.-]*:` starts it.
fn scheme(text: &str) -> Option<&str> {
    let end = text.find(':')?;
    let scheme = &text[..end];
    let mut characters = scheme.chars();
    let first = characters.next()?;
    (first.is_ascii_alphabetic() && characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))).then_some(scheme)
}

/// `%XX` decoded, as Dart's `Uri.decodeComponent`: None for a stray `%` or
/// bytes that aren't UTF-8 (the name stays as written then).
fn percent_decode(text: &str) -> Option<String> {
    if !text.contains('%') {
        return Some(text.to_owned());
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = text.get(at + 1..at + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Whether a `data:` address holds a picture every app reads (PNG, JPEG,
/// GIF, WebP, by its type and its first bytes) of at most 5 MB.
pub(crate) fn data_picture(src: &str) -> bool {
    let text = src.trim();
    if text.len() > DATA_MOST * 4 / 3 + 256 {
        return false;
    }
    let Some(rest) = text.get(..5).filter(|start| start.eq_ignore_ascii_case("data:")).map(|_| &text[5..]) else { return false };
    let Some((meta, data)) = rest.split_once(',') else { return false };
    let mut params = meta.split(';');
    let mime = params.next().unwrap_or("").trim().to_ascii_lowercase();
    if !["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&mime.as_str()) {
        return false;
    }
    let bytes = if params.any(|param| param.trim().eq_ignore_ascii_case("base64")) {
        match base64::engine::general_purpose::STANDARD.decode(data.trim()) {
            Ok(bytes) => bytes,
            Err(_) => return false,
        }
    } else {
        match percent_decode_bytes(data) {
            Some(bytes) => bytes,
            None => return false,
        }
    };
    bytes.len() <= DATA_MOST && picture_type(&bytes)
}

fn percent_decode_bytes(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            out.push(u8::from_str_radix(text.get(at + 1..at + 3)?, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    Some(out)
}

/// Whether `bytes` begin as a PNG, a JPEG, a GIF or a WebP.
fn picture_type(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()))
}

/// Whether the image `src` shows in place in the Markdown file at `file`
/// (in a paragraph or a list item: the caller knows).
pub(crate) fn shown_in_place(file: Option<&str>, src: &str) -> bool {
    if src.trim_start().get(..5).is_some_and(|start| start.eq_ignore_ascii_case("data:")) {
        return data_picture(src);
    }
    // Without the file's place only an absolute one resolves.
    markdown_path(file.unwrap_or(""), src).is_some_and(|path| media_kind(&path).is_some())
}

/// Tracks which block an image is in, from the parser's events: the app
/// shows an image in place only in a paragraph or a list item.
#[derive(Default)]
pub(crate) struct Blocks {
    open: Vec<bool>,
}

impl Blocks {
    /// Takes an event; for an image's start, whether it's in a paragraph or
    /// a list item.
    pub(crate) fn take(&mut self, event: &Event<'_>) -> bool {
        match event {
            Event::Start(Tag::Paragraph | Tag::Item) => self.open.push(true),
            Event::Start(Tag::Heading { .. } | Tag::TableCell | Tag::TableHead | Tag::TableRow) => self.open.push(false),
            Event::End(TagEnd::Paragraph | TagEnd::Item | TagEnd::Heading(_) | TagEnd::TableCell | TagEnd::TableHead | TagEnd::TableRow) => {
                self.open.pop();
            }
            Event::Start(Tag::Image { .. }) => return self.open.last().copied().unwrap_or(false),
            _ => {}
        }
        false
    }
}

/// The local pictures the Markdown file at `file` (its text `source`)
/// shows in place, each as the app names it (`~/…` kept), once, at most
/// `most`.
pub(crate) fn pictures(file: &str, source: &str, most: usize) -> Vec<String> {
    let mut blocks = Blocks::default();
    let mut found: Vec<String> = Vec::new();
    for event in Parser::new_ext(source, options()) {
        let in_place = blocks.take(&event);
        if let Event::Start(Tag::Image { dest_url, .. }) = &event
            && in_place
            && let Some(path) = markdown_path(file, dest_url)
            && media_kind(&path) == Some(Media::Picture)
            && !found.contains(&path)
        {
            found.push(path);
            if found.len() >= most {
                break;
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_names_a_file_as_the_app_resolves_it() {
        let file = "/Users/ann/notes/plan.md";
        assert_eq!(markdown_path(file, "shot.png").as_deref(), Some("/Users/ann/notes/shot.png"));
        assert_eq!(markdown_path(file, "./img/../shot.png?v=2#top").as_deref(), Some("/Users/ann/notes/shot.png"));
        assert_eq!(markdown_path(file, "../a%20b.png").as_deref(), Some("/Users/ann/a b.png"));
        assert_eq!(markdown_path(file, "/tmp/x.png").as_deref(), Some("/tmp/x.png"));
        assert_eq!(markdown_path(file, "~/Desktop/x.png").as_deref(), Some("~/Desktop/x.png"));
        assert_eq!(markdown_path(file, "file:///tmp/a%20b.png").as_deref(), Some("/tmp/a b.png"));
        assert_eq!(markdown_path(file, "https://example.com/x.png"), None);
        assert_eq!(markdown_path(file, "#top"), None);
        assert_eq!(markdown_path("/a.md", "../../x.png"), None, "past the root");
        assert_eq!(media_kind("/x/Shot.PNG"), Some(Media::Picture));
        assert_eq!(media_kind("/x/clip.mov"), Some(Media::Video));
        assert_eq!(media_kind("/x/doc.pdf"), None);
    }

    #[test]
    fn a_data_picture_is_one_by_its_type_and_bytes() {
        let png = base64::engine::general_purpose::STANDARD.encode([0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0]);
        assert!(data_picture(&format!("data:image/png;base64,{png}")));
        assert!(!data_picture(&format!("data:image/svg+xml;base64,{png}")));
        assert!(!data_picture("data:image/png;base64,aGVsbG8="), "not a PNG's bytes");
    }

    #[test]
    fn only_pictures_in_a_paragraph_or_a_list_item_are_watched() {
        let source = "# ![h](head.png)\n\nSee ![a](a.png) and ![b](b.jpg).\n\n- ![c](sub/c.gif)\n\n| t |\n|---|\n| ![d](d.png) |\n\n![e](https://x/e.png) ![f](f.mp4) ![a](a.png)\n";
        assert_eq!(pictures("/n/doc.md", source, 10), ["/n/a.png", "/n/b.jpg", "/n/sub/c.gif"]);
        assert_eq!(pictures("/n/doc.md", source, 2), ["/n/a.png", "/n/b.jpg"]);
    }
}
