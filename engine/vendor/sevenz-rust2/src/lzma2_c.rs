//! Raw LZMA2 encoding through liblzma (`liblzma` feature).
//!
//! Produces the same chunked LZMA2 stream as the pure-Rust writer, at C-library
//! speed. The raw encoder emits exactly the bytes a 7z LZMA2 coder stream must
//! contain (no xz container, no headers); the dictionary property byte stays in
//! the archive coder info, which is derived from the options as before.
//!
//! Block-level parallelism is the caller's job: one `Lzma2CWriter` per stream.
#![cfg(feature = "liblzma")]

use std::io::{self, Write};
use std::mem;

use lzma_sys::*;

/// Settings carried over from the crate's `LzmaOptions`, expressed as plain values
/// so this module never depends on `lzma_rust2` types (which are encoder-gated).
pub struct Lzma2CConfig {
    /// liblzma preset (0-9): defines nice_len, match finder, depth, lc/lp/pb, mode.
    pub preset: u32,
    /// Dictionary size override (the caller's dict setting wins over the preset).
    pub dict_size: u32,
}

pub struct Lzma2CWriter<W: Write> {
    inner: Option<W>,
    strm: lzma_stream,
    out: Vec<u8>,
    finished: bool,
}

impl<W: Write> Lzma2CWriter<W> {
    /// # Panics
    /// Panics if liblzma rejects the filter chain (out-of-range options); the
    /// caller (`add_encoder`) validates dictionary size before reaching here.
    pub fn new(inner: W, c: &Lzma2CConfig) -> Self {
        unsafe {
            let mut opts: lzma_options_lzma = mem::zeroed();
            // Preset first so every field we do not model starts from the value
            // liblzma itself tuned for that level; only the dictionary is overridden.
            assert_eq!(
                u32::from(lzma_lzma_preset(&mut opts, c.preset.min(9))),
                lzma_sys::LZMA_OK
            );
            opts.dict_size = c.dict_size;
            let filters = [
                lzma_filter { id: lzma_sys::LZMA_FILTER_LZMA2, options: &mut opts as *mut _ as *mut core::ffi::c_void },
                lzma_filter { id: lzma_sys::LZMA_VLI_UNKNOWN, options: core::ptr::null_mut() },
            ];
            let mut strm: lzma_stream = mem::zeroed();
            let rc = lzma_raw_encoder(&mut strm, filters.as_ptr());
            if u32::from(rc) != lzma_sys::LZMA_OK {
                lzma_end(&mut strm);
                panic!("liblzma lzma_raw_encoder failed: {rc}");
            }
            // liblzma copies the options during init, so the stack values above are fine here.
            Self { inner: Some(inner), strm, out: vec![0u8; 64 * 1024], finished: false }
        }
    }

    /// Runs the encoder until all pending output is drained. `action` is `LZMA_RUN`
    /// during writes and `LZMA_FINISH` in [`Self::finish`].
    fn pump(&mut self, action: lzma_action) -> io::Result<()> {
        unsafe {
            loop {
                self.strm.next_out = self.out.as_mut_ptr();
                self.strm.avail_out = self.out.len();
                let rc = u32::from(lzma_code(&mut self.strm, action));
                let produced = self.out.len() - self.strm.avail_out as usize;
                if produced > 0 {
                    self.inner.as_mut().unwrap().write_all(&self.out[..produced])?;
                }
                match rc {
                    lzma_sys::LZMA_OK => {
                        if action as u32 == lzma_sys::LZMA_RUN && self.strm.avail_in == 0 && produced == 0 {
                            return Ok(()); // waiting for more input
                        }
                    }
                    lzma_sys::LZMA_STREAM_END => return Ok(()),
                    lzma_sys::LZMA_BUF_ERROR => {
                        if self.strm.avail_in == 0 && produced == 0 && action as u32 == lzma_sys::LZMA_RUN {
                            return Ok(()); // needs input
                        }
                        return Err(io::Error::new(io::ErrorKind::WriteZero, "liblzma: no progress possible"));
                    }
                    code => {
                        return Err(io::Error::new(io::ErrorKind::Other, format!("liblzma error {code}")));
                    }
                }
            }
        }
    }

    pub fn finish(mut self) -> io::Result<W> {
        if !self.finished {
            self.finished = true;
            self.pump(lzma_sys::LZMA_FINISH as lzma_action)?;
            unsafe { lzma_end(&mut self.strm) };
        }
        let mut inner = self.inner.take().unwrap();
        inner.flush()?;
        Ok(inner)
    }
}

impl<W: Write> Write for Lzma2CWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.strm.next_in = buf.as_ptr();
        self.strm.avail_in = buf.len();
        while self.strm.avail_in > 0 {
            self.pump(lzma_sys::LZMA_RUN as lzma_action)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.as_mut().unwrap().flush()
    }
}

impl<W: Write> Drop for Lzma2CWriter<W> {
    fn drop(&mut self) {
        if !self.finished {
            unsafe { lzma_end(&mut self.strm) };
        }
    }
}
