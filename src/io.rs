use core::{fmt, slice};

pub type Result<T> = core::result::Result<T, nc::Errno>;

pub const trait Write: Sized {
    fn write(&mut self, bytes: &[u8]) -> Result<usize>;
    fn flush(&mut self) -> Result<usize>;
    fn write_all(&mut self, bytes: &[u8]) -> Result<()>;

    fn write_u64(&mut self, mut n: u64) -> Result<usize> {
        unsafe {
            let mut buf = core::mem::MaybeUninit::<[u8; 20]>::uninit();
            let buf = buf.assume_init_mut();
            let end = buf.as_mut_ptr_range().end;
            let mut beg = end.offset(-1);
            loop {
                *beg = b'0' + (n % 10) as u8;
                n /= 10;
                if n == 0 {
                    let len = end.offset_from_unsigned(beg);
                    break match self
                        .write_all(slice::from_raw_parts(beg, end.offset_from_unsigned(beg)))
                    {
                        Ok(_) => Ok(len),
                        Err(e) => Err(e),
                    };
                }
                beg = beg.sub(1);
            }
        }
    }
}

pub const STDIN: i32 = 0;
pub const STDOUT: i32 = 1;
pub const STDERR: i32 = 2;

pub struct FdWriter(i32);
#[derive(Clone, Copy)]
pub struct FdReader(i32);

impl FdWriter {
    pub const fn stdout() -> Self {
        Self(STDOUT)
    }
    pub const fn stderr() -> Self {
        Self(STDERR)
    }
}

impl FdReader {
    pub const fn stdin() -> Self {
        Self(STDIN)
    }

    pub fn read(self, buf: &mut [u8]) -> Result<usize> {
        unsafe { nc::read(self.0, buf) }.map(|x| x as _)
    }
}

impl Write for FdWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        unsafe { nc::write(self.0, bytes) }.map(|x| x as _)
    }
    fn flush(&mut self) -> Result<usize> {
        Ok(0)
    }
    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        let mut written = 0;
        while written < bytes.len() {
            written += self.write(unsafe { bytes.get_unchecked(written..) })?;
        }
        Ok(())
    }
}

impl fmt::Write for FdWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_all(s.as_bytes()).map_err(|_| fmt::Error)
    }
}

pub struct BufWriter<Buffer: AsMut<[u8]>, Write: self::Write> {
    writer: Write,
    buffer: Buffer,
    offset: usize,
}

impl<Buffer: AsMut<[u8]>, Write: self::Write> BufWriter<Buffer, Write> {
    pub const fn new(writer: Write, buffer: Buffer) -> Self {
        Self {
            writer,
            buffer,
            offset: 0,
        }
    }

    pub fn flush(&mut self) -> Result<usize> {
        let n = self.offset;
        self.offset = 0;
        self.writer
            .write_all(unsafe { &self.buffer.as_mut().get_unchecked(..n) })?;
        Ok(n)
    }

    fn fill(&mut self, bytes: &[u8]) {
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.buffer.as_mut().as_mut_ptr().add(self.offset),
                bytes.len(),
            )
        };
        self.offset = unsafe { self.offset.unchecked_add(bytes.len()) };
    }

    fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        if self.offset == 0 {
            if bytes.len() > self.buffer.as_mut().len() {
                self.writer.write_all(bytes)?;
                return Ok(bytes.len());
            }
            self.fill(bytes);
            return Ok(bytes.len());
        }
        let remaining = self.buffer.as_mut().len() - self.offset;
        if bytes.len() <= remaining {
            self.fill(bytes);
            return Ok(bytes.len());
        }
        self.fill(unsafe { bytes.get_unchecked(..remaining) });
        self.flush()?;
        self.write(unsafe { bytes.get_unchecked(remaining..) })
    }
}

impl<Buffer: AsMut<[u8]>, Write: self::Write> self::Write for BufWriter<Buffer, Write> {
    fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        self.write(bytes)
    }
    fn flush(&mut self) -> Result<usize> {
        self.flush()
    }
    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.write(bytes).map(|_| ())
    }
}

pub struct ArrayWriter<'a, const N: usize> {
    buf: &'a mut [u8; N],
    pub len: usize,
}

impl<const N: usize> const Write for ArrayWriter<'_, N> {
    fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        unsafe { self.write_bytes_unchecked(bytes) };
        Ok(bytes.len())
    }

    fn flush(&mut self) -> Result<usize> {
        unimplemented!()
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        _ = self.write(bytes);
        Ok(())
    }
}

impl<'a, const N: usize> ArrayWriter<'a, N> {
    pub const fn new(buf: &'a mut [u8; N]) -> Self {
        Self { buf, len: 0 }
    }
    pub const unsafe fn write_byte_unchecked(&mut self, byte: u8) {
        self.buf[self.len] = byte as u8;
        self.len += 1;
    }
    pub const unsafe fn write_bytes_unchecked(&mut self, bytes: &[u8]) {
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.buf.as_mut_ptr().add(self.len),
                bytes.len(),
            );
        }
        self.len += bytes.len();
    }
    pub const unsafe fn write_u64_unchecked(&mut self, n: u64) {
        _ = self.write_u64(n);
    }
}

#[test]
fn test_copy() {
    let src = b"hello";
    let mut dst = [0; 18];
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), src.len()) };
    assert_eq!(dst[..src.len()], src[..])
}

//impl<Buffer: AsMut<[u8]>, Write: self::Write> fmt::Write for BufWriter<Buffer, Write> {
//    fn write_str(&mut self, s: &str) -> fmt::Result {
//        self.write(s.as_bytes()).map(|_| ()).map_err(|_| fmt::Error)
//    }
//}
