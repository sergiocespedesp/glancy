//! Random-access reading of the file, whether it comes from a path or from an Explorer `IStream`.

use std::io;
use std::sync::Mutex;
use windows::Win32::System::Com::{IStream, STATFLAG_NONAME, STREAM_SEEK_SET};

pub trait Source {
    fn len(&self) -> u64;
    /// Reads up to `buf.len()` bytes from `offset`; returns how many were read (0 = end).
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut done = 0;
        while done < buf.len() {
            let n = self.read_at(offset + done as u64, &mut buf[done..])?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected end of file"));
            }
            done += n;
        }
        Ok(())
    }

    /// Reads `len` bytes (or fewer if the file ends sooner).
    fn read_vec(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let avail = self.len().saturating_sub(offset).min(len as u64) as usize;
        let mut v = vec![0; avail];
        self.read_exact_at(offset, &mut v)?;
        Ok(v)
    }

    /// The whole file, with a cap so Explorer's memory is not exhausted.
    fn read_all(&self, limit: u64) -> io::Result<Vec<u8>> {
        if self.len() > limit {
            return Err(io::Error::other(format!("file too large ({} MB)", self.len() / 1_048_576)));
        }
        self.read_vec(0, self.len() as usize)
    }
}

pub struct FileSource {
    file: std::fs::File,
    len: u64,
}

impl FileSource {
    pub fn open(path: &std::path::Path) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        use std::os::windows::fs::FileExt;
        self.file.seek_read(buf, offset)
    }
}

pub struct MemorySource(pub Vec<u8>);

impl Source for MemorySource {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let start = (offset as usize).min(self.0.len());
        let n = buf.len().min(self.0.len() - start);
        buf[..n].copy_from_slice(&self.0[start..start + n]);
        Ok(n)
    }
}

/// A shared `IStream`: each read does Seek + Read under a lock.
pub struct StreamSource {
    stream: Mutex<IStream>,
    len: u64,
}

impl StreamSource {
    pub fn new(stream: IStream) -> windows::core::Result<Self> {
        let mut stat = Default::default();
        unsafe { stream.Stat(&mut stat, STATFLAG_NONAME)? };
        Ok(Self { stream: Mutex::new(stream), len: stat.cbSize })
    }
}

impl Source for StreamSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let stream = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            stream.Seek(offset as i64, STREAM_SEEK_SET, None).map_err(io::Error::other)?;
            let mut total = 0usize;
            // Read may return fewer bytes than requested without being at the end.
            while total < buf.len() {
                let want = (buf.len() - total).min(u32::MAX as usize) as u32;
                let mut read = 0u32;
                let hr = stream.Read(buf[total..].as_mut_ptr() as *mut _, want, Some(&mut read));
                if hr.is_err() {
                    return Err(io::Error::other(windows::core::Error::from(hr)));
                }
                if read == 0 {
                    break;
                }
                total += read as usize;
            }
            Ok(total)
        }
    }
}

/// A buffered sequential reader over a region of a `Source` (to walk PSD channels row by row).
pub struct Cursor<'a> {
    src: &'a dyn Source,
    pos: u64,
    buf: Vec<u8>,
    start: usize,
    end: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(src: &'a dyn Source, pos: u64) -> Self {
        Self { src, pos, buf: vec![0; 64 * 1024], start: 0, end: 0 }
    }

    /// Copies exactly `out.len()` bytes; if the file is truncated, fills with zeros and returns `false`.
    pub fn read(&mut self, out: &mut [u8]) -> io::Result<bool> {
        let mut done = 0;
        while done < out.len() {
            if self.start == self.end {
                let n = self.src.read_at(self.pos, &mut self.buf)?;
                if n == 0 {
                    out[done..].fill(0);
                    return Ok(false);
                }
                self.pos += n as u64;
                self.start = 0;
                self.end = n;
            }
            let n = (self.end - self.start).min(out.len() - done);
            out[done..done + n].copy_from_slice(&self.buf[self.start..self.start + n]);
            self.start += n;
            done += n;
        }
        Ok(true)
    }

    pub fn skip(&mut self, mut n: u64) {
        let in_buf = (self.end - self.start) as u64;
        if n <= in_buf {
            self.start += n as usize;
            return;
        }
        n -= in_buf;
        self.start = 0;
        self.end = 0;
        self.pos += n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_source_reads() {
        let s = MemorySource((0u8..100).collect());
        let mut b = [0u8; 10];
        s.read_exact_at(95, &mut b[..5]).unwrap();
        assert_eq!(&b[..5], &[95, 96, 97, 98, 99]);
        assert!(s.read_exact_at(96, &mut b).is_err());
        assert_eq!(s.read_vec(98, 10).unwrap(), vec![98, 99]);
    }

    #[test]
    fn cursor_reads_and_skips() {
        let s = MemorySource((0u8..=255).cycle().take(200_000).collect());
        let mut c = Cursor::new(&s, 10);
        let mut b = [0u8; 3];
        assert!(c.read(&mut b).unwrap());
        assert_eq!(b, [10, 11, 12]);
        c.skip(100_000);
        assert!(c.read(&mut b).unwrap());
        assert_eq!(b[0], ((100_013) % 256) as u8);
        c.skip(1_000_000);
        assert!(!c.read(&mut b).unwrap());
    }
}
