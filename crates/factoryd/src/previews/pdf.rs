//! Pages of big PDFs as images (#379 step 3): a phone opening a PDF over
//! 5 MB asks for a page (ReadFile.preview_page) and shows it while the PDF
//! itself loads. On a Mac, `claktory-video-preview pdf` renders it with
//! CoreGraphics, filling the screen the way round the page is; factoryd
//! encodes it as WebP. Elsewhere nothing renders PDFs: the answer is
//! FILE_PREVIEW_UNAVAILABLE with the PDF's size and version and none of its
//! bytes, since the app loads the PDF itself by its own requests.
//!
//! Kept as other previews are, one file a page: `<sha>-<screen>-p<page>`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use factory_protocol::v1;

use super::{Header, Screen};
use crate::extensions::viewer;

/// A PDF this big or smaller comes as it is: it loads fast enough.
const PDF_OVER: u64 = 5_000_000;
/// The longest side a page is rendered at (the helper's own limit).
const PAGE_SIDE_MOST: u32 = 4096;
/// How long rendering a page may take.
const RENDERING_MOST: Duration = Duration::from_secs(60);
/// The helper's exit when the page is past the end.
const PAST_THE_END: i32 = 3;

pub(super) fn is_pdf(mime: &str) -> bool {
    mime == "application/pdf"
}

/// Where the image of `page` for `screen` is kept.
fn kept_file(data: &Path, sha: &str, screen: Screen, page: u32) -> PathBuf {
    data.join(super::FOLDER).join(format!("{sha}-{}-p{page}.preview", screen.name()))
}

/// What rendering a page came to.
enum Rendered {
    Page(Header),
    /// It has fewer pages: how many.
    PastTheEnd(u32),
}

/// ReadFile of a PDF with a screen: the page asked for as an image, when the
/// PDF is big; the PDF as before otherwise.
pub(super) fn read(data: &Path, request: &v1::ReadFile, start: &v1::FileContent, screen: Screen) -> v1::FileContent {
    read_with(data, request, start, screen, super::video::helper)
}

/// `read`, with `helper` saying what renders pages here.
pub(super) fn read_with(
    data: &Path,
    request: &v1::ReadFile,
    start: &v1::FileContent,
    screen: Screen,
    helper: impl FnOnce() -> Option<PathBuf>,
) -> v1::FileContent {
    let page = request.preview_page;
    let original = || viewer::read_requested(request);
    if page == 0 || start.size <= PDF_OVER || start.sha256.is_empty() {
        return original();
    }
    // No page: what the PDF is, without its bytes. The page request runs
    // beside the PDF's own loading, so 8 MB of it here would be thrown away
    // and slow that loading down on a slow link.
    let unavailable = || v1::FileContent {
        data: Vec::new(),
        truncated: true,
        preview: v1::FilePreview::Unavailable.into(),
        original_size: start.size,
        ..start.clone()
    };
    let path = PathBuf::from(&start.path);
    let file = kept_file(data, &start.sha256, screen, page);
    let header = match super::header_of(&file) {
        Some(header) => header,
        None => {
            let Some(helper) = helper() else { return unavailable() };
            match render(data, &path, &start.sha256, screen, page, &helper) {
                Ok(Rendered::Page(header)) => header,
                Ok(Rendered::PastTheEnd(pages)) => {
                    let s = if pages == 1 { "" } else { "s" };
                    return v1::FileContent {
                        path: start.path.clone(),
                        error: format!("This PDF has {pages} page{s}."),
                        page_count: pages,
                        ..v1::FileContent::default()
                    };
                }
                Err(why) => {
                    tracing::info!("no preview of page {page} of {}: {why}", path.display());
                    return unavailable();
                }
            }
        }
    };
    super::touch(&file);
    match super::range(&file, 0, u64::MAX) {
        Some((webp, size)) => v1::FileContent {
            path: start.path.clone(),
            mime: "image/webp".to_owned(),
            size,
            data: webp,
            modified_ms: start.modified_ms,
            sha256: start.sha256.clone(),
            preview: v1::FilePreview::Made.into(),
            pixel_width: header.width,
            pixel_height: header.height,
            original_size: start.size,
            original_pixel_width: header.original_width,
            original_pixel_height: header.original_height,
            page_count: header.pages,
            ..v1::FileContent::default()
        },
        None => unavailable(),
    }
}

/// An agent showed a PDF: its first page for `screens`, when it's big.
pub(super) fn ahead(data: &Path, path: &Path, sha: &str, size: u64, screens: &[Screen]) {
    let Some(helper) = super::video::helper() else { return };
    if size <= PDF_OVER {
        return;
    }
    for screen in screens {
        if super::header_of(&kept_file(data, sha, *screen, 1)).is_none()
            && let Err(why) = render(data, path, sha, *screen, 1, &helper)
        {
            tracing::info!("no preview of page 1 of {}: {why}", path.display());
        }
    }
}

/// Renders and keeps `page` of the PDF at `path` (version `sha`) for
/// `screen`.
fn render(data: &Path, path: &Path, sha: &str, screen: Screen, page: u32, helper: &Path) -> Result<Rendered, String> {
    let _making = super::MAKING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let file = kept_file(data, sha, screen, page);
    if let Some(header) = super::header_of(&file) {
        return Ok(Rendered::Page(header));
    }
    let folder = data.join(super::FOLDER);
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let job = super::Job::new(&folder).map_err(|error| error.to_string())?;
    let png = job.file("page.png");
    let (short, long) = (screen.short.min(PAGE_SIDE_MOST), screen.long.min(PAGE_SIDE_MOST));
    let mut command = Command::new(helper);
    command.arg("pdf").arg(path).arg(&png).arg(page.to_string()).arg(short.to_string()).arg(long.to_string());
    let ran = super::run(&mut command, RENDERING_MOST);
    let said: serde_json::Value = ran.as_ref().ok().and_then(|ran| serde_json::from_str(ran.out.trim()).ok()).unwrap_or_default();
    let number = |key: &str| said.get(key).and_then(serde_json::Value::as_u64).and_then(|value| u32::try_from(value).ok());
    let rendered = match ran {
        Err(why) => Err(why),
        Ok(ran) if ran.status.code() == Some(PAST_THE_END) => match number("pages") {
            Some(pages) => return Ok(Rendered::PastTheEnd(pages)),
            None => Err("it said the page was past the end, but not how many there are".to_owned()),
        },
        Ok(ran) if !ran.status.success() => Err(format!("{}: {}", ran.status, ran.said.trim())),
        Ok(_) => std::fs::read(&png).map_err(|error| error.to_string()).and_then(|bytes| {
            let picture = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).map_err(|error| error.to_string())?;
            let webp = super::webp(&picture)?;
            let header = Header {
                width: picture.width(),
                height: picture.height(),
                original_width: number("page_width").unwrap_or(0),
                original_height: number("page_height").unwrap_or(0),
                pages: number("pages").unwrap_or(0),
            };
            Ok((header, webp))
        }),
    };
    drop(job);
    let (header, webp) = rendered?;
    // Kept only as the version asked about.
    if viewer::whole_sha256(path).is_ok_and(|now| now == sha) {
        super::keep_bytes(&folder, &file, &header, &webp).map_err(|error| error.to_string())?;
    } else {
        return Err("it changed while its page was rendered".to_owned());
    }
    Ok(Rendered::Page(header))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: Screen = Screen { short: 1320, long: 2868 };

    /// A stand-in for the helper's `pdf` mode: a page as the real one
    /// renders it, or past the end of a 4-page PDF, counting its runs.
    fn helper(dir: &Path) -> PathBuf {
        let png = dir.join("page.png");
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(1320, 1708, |x, y| image::Rgb([(x % 251) as u8, (y % 241) as u8, 200])))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(&png, bytes).unwrap();
        let script = dir.join("helper");
        let body = format!(
            "#!/bin/sh\necho \"$*\" >> '{0}/runs'\n[ \"$1\" = pdf ] || exit 1\nif [ \"$4\" -gt 4 ]; then echo '{{\"pages\": 4}}'; exit 3; fi\ncp '{1}' \"$3\"\necho '{{\"width\": 1320, \"height\": 1708, \"pages\": 4, \"page_width\": 612, \"page_height\": 792}}'\n",
            dir.display(),
            png.display()
        );
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        script
    }

    fn ask(path: &Path, page: u32) -> (v1::ReadFile, v1::FileContent) {
        let request = v1::ReadFile {
            path: path.display().to_string(),
            preview_width: PHONE.short,
            preview_height: PHONE.long,
            preview_page: page,
            ..v1::ReadFile::default()
        };
        let start = viewer::read_requested(&v1::ReadFile { path: request.path.clone(), max_bytes: 512, ..v1::ReadFile::default() });
        (request, start)
    }

    #[test]
    fn a_big_pdfs_page_comes_as_an_image_while_a_small_one_comes_as_it_is() {
        let dir = std::env::temp_dir().join(format!("claktory-pdf-previews-{}", std::process::id()));
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let big = dir.join("scan.pdf");
        std::fs::write(&big, [b"%PDF-1.4\n".as_slice(), &vec![b'x'; 6_000_000]].concat()).unwrap();
        let helper = helper(&dir);

        let (request, start) = ask(&big, 1);
        let page = read_with(&data, &request, &start, PHONE, || Some(helper.clone()));
        assert_eq!(page.error, "");
        assert_eq!(page.preview(), v1::FilePreview::Made);
        assert_eq!((page.mime.as_str(), page.size, page.truncated), ("image/webp", page.data.len() as u64, false));
        assert_eq!((page.pixel_width, page.pixel_height, page.page_count), (1320, 1708, 4));
        assert_eq!((page.original_pixel_width, page.original_pixel_height), (612, 792), "the page in points");
        assert_eq!((page.original_size, page.sha256 == start.sha256), (start.size, true));
        let shown = image::load_from_memory_with_format(&page.data, image::ImageFormat::WebP).unwrap();
        assert_eq!((shown.width(), shown.height()), (1320, 1708));
        // Kept: asked again, it isn't rendered again.
        let again = read_with(&data, &request, &start, PHONE, || panic!("rendered again"));
        assert_eq!(again.data, page.data);
        assert_eq!(std::fs::read_to_string(dir.join("runs")).unwrap().lines().count(), 1);

        // Past the end: how many pages it has.
        let (request, start) = ask(&big, 9);
        let past = read_with(&data, &request, &start, PHONE, || Some(helper.clone()));
        assert_eq!((past.error.as_str(), past.page_count, past.data.len()), ("This PDF has 4 pages.", 4, 0));

        // Nothing renders pages here: says so, with what the PDF is and
        // none of its bytes (the app loads the PDF by its own requests).
        let (request, start) = ask(&big, 2);
        let none = read_with(&data, &request, &start, PHONE, || None);
        assert_eq!((none.preview(), none.mime.as_str(), none.offset), (v1::FilePreview::Unavailable, "application/pdf", 0));
        assert_eq!((none.size, none.original_size, none.sha256.as_str(), none.data.len()), (start.size, start.size, start.sha256.as_str(), 0));
        assert_eq!(none.error, "");

        // A small PDF, or no page asked: the PDF as before.
        let small = dir.join("notes.pdf");
        std::fs::write(&small, [b"%PDF-1.4\n".as_slice(), &vec![b'x'; 100_000]].concat()).unwrap();
        let (request, start) = ask(&small, 1);
        let content = read_with(&data, &request, &start, PHONE, || panic!("too small to render"));
        assert_eq!((content.preview(), content.mime.as_str(), content.size), (v1::FilePreview::None, "application/pdf", start.size));
        let (request, start) = ask(&big, 0);
        let content = read_with(&data, &request, &start, PHONE, || panic!("no page asked"));
        assert_eq!((content.preview(), content.mime.as_str()), (v1::FilePreview::None, "application/pdf"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
