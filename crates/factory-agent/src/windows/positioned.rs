//! Reading and writing at a place in a file without a file of one's own
//! position: `std::os::unix::fs::FileExt`'s `read_at` and `write_all_at`,
//! for `tools::task_logs`' cuts. On Windows they move the handle's own
//! position, which nothing else uses (each cut opens the log itself), and a
//! task's appends go to the end whatever any handle's position is.

use std::os::windows::fs::FileExt as _;

pub trait FileExt {
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> std::io::Result<usize>;
    fn write_all_at(&self, buffer: &[u8], offset: u64) -> std::io::Result<()>;
}

impl FileExt for std::fs::File {
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
        match self.seek_read(buffer, offset) {
            // Reading past the end is an error on Windows, nothing elsewhere.
            Err(error) if error.raw_os_error() == Some(38 /* ERROR_HANDLE_EOF */) => Ok(0),
            other => other,
        }
    }

    fn write_all_at(&self, mut buffer: &[u8], mut offset: u64) -> std::io::Result<()> {
        while !buffer.is_empty() {
            match self.seek_write(buffer, offset) {
                Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
                Ok(written) => {
                    buffer = &buffer[written..];
                    offset += written as u64;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::FileExt;

    #[test]
    fn reads_and_writes_at_a_place_and_past_the_end_reads_nothing() {
        let path = std::env::temp_dir().join(format!("claktory-positioned-{}", std::process::id()));
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
        file.write_all_at(b"hello world", 0).unwrap();
        file.write_all_at(b"W", 6).unwrap();
        let mut buffer = [0u8; 5];
        assert_eq!(file.read_at(&mut buffer, 6).unwrap(), 5);
        assert_eq!(&buffer, b"World");
        assert_eq!(file.read_at(&mut buffer, 100).unwrap(), 0);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }
}
