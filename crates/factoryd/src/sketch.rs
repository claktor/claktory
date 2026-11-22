//! Sketches (docs/ipad-phase3-contract.md §6): a blank white PNG in one of
//! the person's projects' folders, to draw on. It's an ordinary image file
//! from then on, with an ink layer on it ("blank_canvas"), discussed through
//! the same threads.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use factory_protocol::v1;
use sha2::{Digest, Sha256};

use crate::factory::{Factory, RequestError};

/// A sketch's sides, at most, in pixels.
const SIDE_MOST: u32 = 8192;
/// The longest name.
const NAME_MOST: usize = 200;
/// How many " 2", " 3"… are tried when the name is taken.
const TRIES: u32 = 1000;

fn refused(why: impl Into<String>) -> RequestError {
    RequestError::Ink(why.into())
}

/// Passes what's written on to a file, hashing it.
struct Hashing {
    file: std::fs::File,
    hash: Sha256,
}

impl Write for Hashing {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.file.write(bytes)?;
        self.hash.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Writes a white `width` by `height` PNG (8-bit grey) to `file`, durably:
/// its SHA-256.
fn write_white(file: std::fs::File, width: u32, height: u32) -> std::io::Result<String> {
    let mut out = Hashing { file, hash: Sha256::new() };
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Balanced);
        let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
        let mut stream = writer.stream_writer().map_err(std::io::Error::other)?;
        let row = vec![0xff_u8; width as usize];
        for _ in 0..height {
            stream.write_all(&row)?;
        }
        stream.finish().map_err(std::io::Error::other)?;
        writer.finish().map_err(std::io::Error::other)?;
    }
    out.file.sync_all()?;
    Ok(crate::encoding::hex(&out.hash.finalize()))
}

/// `name`, `name 2`, `name 3`…: the first that isn't taken, made empty.
fn claim(directory: &Path, name: &str) -> Result<(PathBuf, std::fs::File), RequestError> {
    for n in 1..=TRIES {
        let file = if n == 1 { format!("{name}.png") } else { format!("{name} {n}.png") };
        let path = directory.join(file);
        // Never over anything there, a link included.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(refused(format!("Claktory can't make a sketch in {}: {error}", directory.display()))),
        }
    }
    Err(refused(format!("There are already {TRIES} sketches named \"{name}\" there; choose another name.")))
}

/// CreateSketch.
pub(crate) async fn create(factory: &Arc<Factory>, request: v1::CreateSketch) -> Result<v1::SketchCreated, RequestError> {
    let name = request.name.trim().to_owned();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(refused("A sketch's name can't be empty, \".\" or \"..\", or have a \"/\" in it."));
    }
    if name.len() > NAME_MOST {
        return Err(refused(format!("A sketch's name is at most {NAME_MOST} characters.")));
    }
    let (width, height) = (request.width, request.height);
    if !(1..=SIDE_MOST).contains(&width) || !(1..=SIDE_MOST).contains(&height) {
        return Err(refused(format!("A sketch is 1 to {SIDE_MOST} pixels on each side.")));
    }
    let asked = request.directory.trim();
    if asked.is_empty() {
        return Err(RequestError::Empty("directory"));
    }
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let expanded = match asked {
        "~" => home,
        _ => asked.strip_prefix("~/").map_or_else(|| PathBuf::from(asked), |rest| home.join(rest)),
    };
    if !expanded.is_absolute() {
        return Err(RequestError::RelativePath(asked.to_owned()));
    }
    let directory = std::fs::canonicalize(&expanded)
        .ok()
        .filter(|directory| directory.is_dir())
        .ok_or_else(|| refused(format!("Claktory can't find the folder {}.", expanded.display())))?;
    // Only in the person's projects' folders, and never in Claktory's own
    // private storage.
    let roots: Vec<PathBuf> = {
        let inner = factory.lock();
        inner
            .config
            .projects
            .iter()
            .flat_map(|project| std::iter::once(project.path.clone()).chain(project.folders.iter().map(|folder| folder.path.clone())))
            .collect()
    };
    let inside = roots.iter().filter_map(|root| std::fs::canonicalize(root).ok()).any(|root| directory.starts_with(root));
    if !inside {
        return Err(refused("A sketch goes in one of your projects' folders."));
    }
    let private = crate::ink::root(&factory.home);
    if std::fs::canonicalize(&private).is_ok_and(|private| directory.starts_with(private)) {
        return Err(refused("That folder is Claktory's own storage; choose one of your project's folders."));
    }
    tokio::task::spawn_blocking(move || {
        let (path, file) = claim(&directory, &name)?;
        match write_white(file, width, height) {
            Ok(sha256) => Ok(v1::SketchCreated { path: path.display().to_string(), sha256 }),
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                Err(refused(format!("Claktory couldn't write the sketch: {error}")))
            }
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sketch_is_a_white_png_of_its_size() {
        let dir = std::env::temp_dir().join(format!("factory-sketch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let (path, file) = claim(&dir, "Idea").expect("claimed");
        let sha = write_white(file, 300, 200).expect("written");
        let bytes = std::fs::read(&path).expect("read");
        assert_eq!(crate::encoding::hex(&Sha256::digest(&bytes)), sha);
        assert_eq!(crate::ink::png_size(&bytes), Some((300, 200)));
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("a PNG");
        let mut pixels = vec![0; reader.output_buffer_size().expect("size")];
        reader.next_frame(&mut pixels).expect("decoded");
        assert!(pixels.iter().all(|pixel| *pixel == 0xff), "white");
        // Taken: " 2", then " 3".
        assert_eq!(claim(&dir, "Idea").expect("claimed").0, dir.join("Idea 2.png"));
        assert_eq!(claim(&dir, "Idea").expect("claimed").0, dir.join("Idea 3.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
