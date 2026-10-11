//! Stream codecs for the TAR family and single-stream formats, with parallel writers
//! (pigz-style gzip, pbzip2-style bzip2, multithreaded xz/zstd).
use crate::error::{Error, ErrorKind, Result};
use crate::zip::Level;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec { None, Gzip, Bzip2, Xz, Lzma, Zstd, Lz4, Brotli }

impl Codec {
    pub fn ext(self) -> &'static str {
        match self { Codec::None => "", Codec::Gzip => "gz", Codec::Bzip2 => "bz2", Codec::Xz => "xz", Codec::Lzma => "lzma", Codec::Zstd => "zst", Codec::Lz4 => "lz4", Codec::Brotli => "br" }
    }
    pub fn from_ext(e: &str) -> Option<Codec> {
        Some(match e.to_ascii_lowercase().as_str() {
            "gz" | "gzip" | "tgz" => Codec::Gzip, "bz2" | "bz" | "tbz" | "tbz2" => Codec::Bzip2, "xz" | "txz" => Codec::Xz,
            "lzma" | "tlz" => Codec::Lzma, "zst" | "zstd" | "tzst" => Codec::Zstd, "lz4" | "tlz4" => Codec::Lz4, "br" | "tbr" => Codec::Brotli, _ => return None,
        })
    }
    pub fn from_magic(b: &[u8]) -> Option<Codec> {
        if b.starts_with(&[0x1f, 0x8b]) { Some(Codec::Gzip) }
        else if b.starts_with(b"BZh") { Some(Codec::Bzip2) }
        else if b.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0]) { Some(Codec::Xz) }
        else if b.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) { Some(Codec::Zstd) }
        else if b.starts_with(&[0x04, 0x22, 0x4D, 0x18]) { Some(Codec::Lz4) }
        else if b.len() >= 13 && b[0] == 0x5D && b[1..5].iter().any(|_| true) && b[12] == 0 || (b.len() >= 5 && b[0] == 0x5D && b[1] == 0 && b[2] == 0) { Some(Codec::Lzma) }
        else { None }
    }
}

pub trait FinishWrite: Write + Send {
    fn finish(self: Box<Self>) -> io::Result<()>;
}

struct Plain<W: Write + Send>(W);
impl<W: Write + Send> Write for Plain<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> { self.0.write(b) }
    fn flush(&mut self) -> io::Result<()> { self.0.flush() }
}
impl<W: Write + Send> FinishWrite for Plain<W> { fn finish(mut self: Box<Self>) -> io::Result<()> { self.0.flush() } }

// ---- pigz/pbzip2-style parallel block writer
type BlockFn = dyn Fn(&[u8]) -> Vec<u8> + Send + Sync;

struct ParBlocks<W: Write + Send> {
    inner: W,
    pool: Arc<rayon::ThreadPool>,
    f: Arc<BlockFn>,
    buf: Vec<u8>,
    block: usize,
    q: VecDeque<crossbeam_channel::Receiver<(Vec<u8>, crc32fast::Hasher)>>,
    cap: usize,
    crc: crc32fast::Hasher,
    total: u64,
    header: Option<Vec<u8>>,
    gzip: bool,
}

impl<W: Write + Send> ParBlocks<W> {
    fn drain_one(&mut self) -> io::Result<()> {
        if let Some(rx) = self.q.pop_front() {
            let (out, h) = rx.recv().map_err(|_| io::Error::other("codec worker failed"))?;
            if self.gzip { self.crc.combine(&h); }
            self.inner.write_all(&out)?;
        }
        Ok(())
    }
    fn dispatch(&mut self) -> io::Result<()> {
        if self.buf.is_empty() { return Ok(()); }
        if let Some(h) = self.header.take() { self.inner.write_all(&h)?; }
        let data = std::mem::replace(&mut self.buf, Vec::with_capacity(self.block));
        self.total += data.len() as u64;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let f = self.f.clone();
        let gz = self.gzip;
        self.pool.spawn(move || {
            let mut h = crc32fast::Hasher::new();
            if gz { h.update(&data); }
            let _ = tx.send((f(&data), h));
        });
        self.q.push_back(rx);
        while self.q.len() > self.cap { self.drain_one()?; }
        Ok(())
    }
}
impl<W: Write + Send> Write for ParBlocks<W> {
    fn write(&mut self, mut d: &[u8]) -> io::Result<usize> {
        let n = d.len();
        while !d.is_empty() {
            let room = self.block - self.buf.len();
            let k = room.min(d.len());
            self.buf.extend_from_slice(&d[..k]);
            d = &d[k..];
            if self.buf.len() == self.block { self.dispatch()?; }
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
impl<W: Write + Send> FinishWrite for ParBlocks<W> {
    fn finish(mut self: Box<Self>) -> io::Result<()> {
        self.dispatch()?;
        while !self.q.is_empty() { self.drain_one()?; }
        if let Some(h) = self.header.take() { self.inner.write_all(&h)?; }
        if self.gzip {
            self.inner.write_all(&[0x03, 0x00])?; // final empty block
            self.inner.write_all(&self.crc.clone().finalize().to_le_bytes())?;
            self.inner.write_all(&(self.total as u32).to_le_bytes())?;
        }
        self.inner.flush()
    }
}

macro_rules! simple_finish {
    ($name:ident, $t:ty, $fin:expr) => {
        struct $name<W: Write + Send>($t);
        impl<W: Write + Send> Write for $name<W> {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> { self.0.write(b) }
            fn flush(&mut self) -> io::Result<()> { self.0.flush() }
        }
        impl<W: Write + Send> FinishWrite for $name<W> {
            fn finish(self: Box<Self>) -> io::Result<()> { let f: fn($t) -> io::Result<()> = $fin; f(self.0) }
        }
    };
}
simple_finish!(Xz, xz2::write::XzEncoder<W>, |e| e.finish().and_then(|mut w| w.flush()));
simple_finish!(Zst, zstd::stream::write::Encoder<'static, W>, |e| e.finish().and_then(|mut w| w.flush()));
simple_finish!(Lz4, lz4_flex::frame::FrameEncoder<W>, |e| e.finish().map_err(io::Error::other).and_then(|mut w| w.flush()));
struct Br<W: Write + Send>(brotli::CompressorWriter<W>);
impl<W: Write + Send> Write for Br<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> { self.0.write(b) }
    fn flush(&mut self) -> io::Result<()> { self.0.flush() }
}
impl<W: Write + Send> FinishWrite for Br<W> {
    fn finish(mut self: Box<Self>) -> io::Result<()> { self.0.flush()?; let mut w = self.0.into_inner(); w.flush() }
}

pub fn level_idx(l: Level) -> usize { match l { Level::Store => 0, Level::Fast => 1, Level::Normal => 2, Level::Maximum => 3 } }

/// Wrap `w` in a compressing writer. `threads` = 0 means auto.
pub fn compressor<W: Write + Send + 'static>(codec: Codec, level: Level, threads: usize, w: W) -> Result<Box<dyn FinishWrite>> {
    let th = if threads == 0 { crate::sys::perf_cores().max(1) } else { threads };
    let li = level_idx(level);
    Ok(match codec {
        Codec::None => Box::new(Plain(w)),
        Codec::Gzip => {
            let lvl = [0u32, 3, 6, 9][li];
            let pool = Arc::new(crate::sys::build_pool(th));
            let f: Arc<BlockFn> = Arc::new(move |d: &[u8]| {
                use flate2::{Compress, Compression, FlushCompress};
                let mut c = Compress::new(Compression::new(lvl), false);
                let mut out = Vec::with_capacity(d.len() / 2 + 4096);
                let mut input = d;
                loop {
                    let before = c.total_in();
                    if out.capacity() - out.len() < 64 { out.reserve(out.capacity() / 2 + 4096); }
                    c.compress_vec(input, &mut out, FlushCompress::Full).expect("deflate");
                    input = &input[(c.total_in() - before) as usize..];
                    if input.is_empty() && out.len() < out.capacity() { break; }
                }
                out
            });
            // minimal gzip header: no name, mtime 0 (reproducible), OS=3 (unix)
            let header = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, if li == 3 { 2 } else if li == 1 { 4 } else { 0 }, 3];
            let block = if li == 3 { 4 << 20 } else { 1 << 20 };
            Box::new(ParBlocks { inner: w, pool, f, buf: Vec::with_capacity(block), block, q: VecDeque::new(), cap: th * 2, crc: crc32fast::Hasher::new(), total: 0, header: Some(header), gzip: true })
        }
        Codec::Bzip2 => {
            let lvl = [1u32, 1, 6, 9][li];
            let pool = Arc::new(crate::sys::build_pool(th));
            let f: Arc<BlockFn> = Arc::new(move |d: &[u8]| {
                let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(lvl));
                e.write_all(d).unwrap();
                e.finish().unwrap()
            });
            let block = (lvl as usize) * 100_000;
            Box::new(ParBlocks { inner: w, pool, f, buf: Vec::with_capacity(block), block, q: VecDeque::new(), cap: th * 2, crc: crc32fast::Hasher::new(), total: 0, header: None, gzip: false })
        }
        Codec::Xz => {
            let preset = [0u32, 1, 5, 9][li];
            let stream = xz2::stream::MtStreamBuilder::new().threads(th as u32).preset(preset).check(xz2::stream::Check::Crc64).encoder().map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
            Box::new(Xz(xz2::write::XzEncoder::new_stream(w, stream)))
        }
        Codec::Lzma => {
            let preset = [0u32, 1, 5, 9][li];
            let opts = xz2::stream::LzmaOptions::new_preset(preset).map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
            let stream = xz2::stream::Stream::new_lzma_encoder(&opts).map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
            Box::new(Xz(xz2::write::XzEncoder::new_stream(w, stream)))
        }
        Codec::Zstd => {
            let lvl = [1, 3, 9, 19][li];
            let mut e = zstd::stream::write::Encoder::new(w, lvl)?;
            let _ = e.multithread(th as u32);
            Box::new(Zst(e))
        }
        Codec::Lz4 => Box::new(Lz4(lz4_flex::frame::FrameEncoder::new(w))),
        Codec::Brotli => {
            let q = [0u32, 4, 8, 11][li];
            Box::new(Br(brotli::CompressorWriter::new(w, 1 << 16, q, 22)))
        }
    })
}

/// Decompressing reader. `mem_limit` bounds LZMA dictionaries (FR-207).
pub fn decompressor<R: Read + Send + 'static>(codec: Codec, r: R, mem_limit: u64) -> Result<Box<dyn Read + Send>> {
    Ok(match codec {
        Codec::None => Box::new(r),
        Codec::Gzip => Box::new(flate2::read::MultiGzDecoder::new(r)),
        Codec::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(r)),
        Codec::Xz => {
            let s = xz2::stream::Stream::new_stream_decoder(mem_limit.max(1 << 20), xz2::stream::CONCATENATED).map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
            Box::new(xz2::read::XzDecoder::new_stream(r, s))
        }
        Codec::Lzma => {
            let s = xz2::stream::Stream::new_lzma_decoder(mem_limit.max(1 << 20)).map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
            Box::new(xz2::read::XzDecoder::new_stream(r, s))
        }
        Codec::Zstd => {
            let mut d = zstd::stream::read::Decoder::new(r)?;
            let wl = (64 - (mem_limit.max(1 << 20)).leading_zeros() - 1).clamp(10, 31);
            let _ = d.window_log_max(wl);
            Box::new(d)
        }
        Codec::Lz4 => Box::new(lz4_flex::frame::FrameDecoder::new(r)),
        Codec::Brotli => Box::new(brotli::Decompressor::new(r, 1 << 16)),
    })
}

/// Map an I/O error raised by a decoder into the stable taxonomy.
pub fn map_decode_err(e: io::Error) -> Error {
    let s = e.to_string().to_lowercase();
    if s.contains("memory") { Error::new(ErrorKind::DictionaryTooLarge, e.to_string()) } else { Error::new(ErrorKind::BrokenArchive, e.to_string()) }
}
