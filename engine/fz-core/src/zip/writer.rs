//! Parallel, Windows-compatible ZIP writer.
//!
//! Pipeline: orchestrator (reads + dispatches to the rayon codec pool) -> bounded channel ->
//! single writer thread, the only stage that mutates archive structure (no locks on metadata).
use super::crypto::{ AesCtx, ZipCrypto};
use super::format::*;
use crate::cancel::{CancelToken, Progress};
use crate::codepage;
use crate::enumerate::{EntryDesc, EntryKind};
use crate::error::{Error, ErrorKind, Result};
use crate::skiplist::SkipList;
use std::cell::RefCell;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level { Store, Fast, Normal, Maximum }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method { Store, Deflate, Zstd, Bzip2, Lzma, Ppmd }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HighSpeed { Off, On, Aggressive }

#[derive(Clone, Debug)]
pub enum Encryption {
    None,
    /// Legacy; flagged weak in UI.
    ZipCrypto(String),
    /// WinZip AE-2; strength 1=128, 2=192, 3=256.
    Aes { strength: u8, password: String },
}

#[derive(Clone, Debug)]
pub struct ZipOptions {
    pub method: Method,
    pub level: Level,
    pub encryption: Encryption,
    pub high_speed: HighSpeed,
    pub skip: SkipList,
    /// 0 = auto (performance cores x 2)
    pub threads: usize,
    pub comment: String,
    /// None = UTF-8 (EFS + 0x7075). Some(id) = legacy single code page, no EFS.
    pub legacy_codepage: Option<String>,
    pub split_size: Option<u64>,
    /// Bound for in-flight buffers.
    pub memory_budget: u64,
}

impl Default for ZipOptions {
    fn default() -> Self {
        ZipOptions {
            method: Method::Deflate, level: Level::Normal, encryption: Encryption::None,
            high_speed: HighSpeed::On, skip: SkipList::default(), threads: 0, comment: String::new(),
            legacy_codepage: None, split_size: None, memory_budget: crate::sys::default_memory_budget(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ZipStats { pub entries: u64, pub bytes_in: u64, pub bytes_out: u64, pub volumes: u32, pub stored_by_skiplist: u64 }

const LARGE: u64 = 16 << 20;
const BLOCK: usize = 1 << 20;
const ZIP64_HINT: u64 = 0xF000_0000;
pub const MIN_SPLIT: u64 = 100 * 1024;

// ---------------------------------------------------------------- sink

struct Sink {
    out: PathBuf,
    split: Option<u64>,
    disk: u32,
    pos: u64,
    w: Option<BufWriter<File>>,
    parts: Vec<PathBuf>,
    total: u64,
}

impl Sink {
    fn vol_final(out: &Path, disk: u32, last: bool) -> PathBuf {
        if last { out.to_path_buf() } else {
            let mut s = out.to_path_buf().into_os_string();
            let st = out.with_extension("");
            s.clear();
            s.push(st.as_os_str());
            s.push(format!(".z{:02}", disk + 1));
            PathBuf::from(s)
        }
    }
    fn open(out: &Path, split: Option<u64>) -> Result<Sink> {
        let mut s = Sink { out: out.to_path_buf(), split, disk: 0, pos: 0, w: None, parts: vec![], total: 0 };
        s.open_disk()?;
        if split.is_some() { s.write(&SIG_SPAN.to_le_bytes())?; }
        Ok(s)
    }
    fn open_disk(&mut self) -> Result<()> {
        let parent = self.out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let temporary = tempfile::Builder::new().prefix(".osz-zip-").tempfile_in(parent)?;
        let (f, p) = temporary.keep().map_err(|e| Error::from(e.error))?;
        self.parts.push(p);
        self.w = Some(BufWriter::with_capacity(1 << 20, f));
        self.pos = 0;
        Ok(())
    }
    fn roll(&mut self) -> Result<()> {
        self.w.take().unwrap().flush()?;
        self.disk += 1;
        self.open_disk()
    }
    fn ensure_room(&mut self, n: u64) -> Result<()> {
        if let Some(s) = self.split { if s - self.pos < n && self.pos > 0 { self.roll()?; } }
        Ok(())
    }
    fn write(&mut self, mut d: &[u8]) -> Result<()> {
        while !d.is_empty() {
            let n = match self.split {
                Some(s) => {
                    if self.pos >= s { self.roll()?; }
                    ((s - self.pos) as usize).min(d.len())
                }
                None => d.len(),
            };
            self.w.as_mut().unwrap().write_all(&d[..n])?;
            self.pos += n as u64;
            self.total += n as u64;
            d = &d[n..];
        }
        Ok(())
    }
    fn at(&self) -> (u32, u64) { (self.disk, self.pos) }
    /// Flush, fsync once, rename `.part` files to final names.
    fn finish(mut self) -> Result<u32> {
        let mut w = self.w.take().unwrap();
        w.flush()?;
        w.get_ref().sync_all()?;
        drop(w);
        let n = self.parts.len();
        for (i, p) in self.parts.iter().enumerate() {
            let fin = if self.split.is_some() { Self::vol_final(&self.out, i as u32, i + 1 == n) } else { self.out.clone() };
            std::fs::rename(p, fin)?;
        }
        Ok(n as u32)
    }
    fn abort(&mut self) {
        self.w.take();
        for p in &self.parts { let _ = std::fs::remove_file(p); }
    }
}

// ---------------------------------------------------------------- headers

#[derive(Clone)]
struct Hdr {
    name: Vec<u8>,
    unicode_extra: bool,
    method: u16,
    real_method: u16,
    flags: u16,
    mtime: i64,
    mode: u32,
    is_dir: bool,
    crc: u32,
    csize: u64,
    usize: u64,
    aes_strength: Option<u8>,
    local_zip64: bool,
}

impl Hdr {
    fn version_needed(&self) -> u16 {
        if self.aes_strength.is_some() { 51 } else if self.local_zip64 { 45 } else {
            match self.real_method { M_ZSTD => 63, M_BZIP2 => 46, M_STORE => if self.is_dir { 20 } else { 10 }, _ => 20 }
        }
    }
    fn ut_extra(&self) -> Vec<u8> {
        if self.mtime < 0 || self.mtime > u32::MAX as i64 { return vec![]; }
        let mut e = Le::default();
        e.u16(EXTRA_UT).u16(5).bytes(&[1]).u32(self.mtime as u32);
        e.0
    }
    fn uni_extra(&self) -> Vec<u8> {
        if !self.unicode_extra { return vec![]; }
        let mut e = Le::default();
        e.u16(EXTRA_UNICODE_PATH).u16(5 + self.name.len() as u16).bytes(&[1]).u32(crc32fast::hash(&self.name)).bytes(&self.name);
        e.0
    }
    fn aes_extra(&self) -> Vec<u8> {
        match self.aes_strength {
            Some(s) => { let mut e = Le::default(); e.u16(EXTRA_AES).u16(7).u16(2).bytes(b"AE").bytes(&[s]).u16(self.real_method); e.0 }
            None => vec![],
        }
    }
    fn ext_attr(&self) -> u32 {
        let dos: u32 = if self.is_dir { 0x10 } else { 0x20 } | if self.mode & 0o200 == 0 { 1 } else { 0 };
        ((self.mode & 0xFFFF) << 16) | dos
    }
    fn crc_field(&self) -> u32 { if self.aes_strength.is_some() { 0 } else { self.crc } }

    fn local(&self, descriptor: bool) -> Vec<u8> {
        let (dt, dd, _) = dos_datetime(self.mtime);
        let mut extra = Le::default();
        if self.local_zip64 { extra.u16(EXTRA_ZIP64).u16(16).u64(if descriptor { 0 } else { self.usize }).u64(if descriptor { 0 } else { self.csize }); }
        extra.bytes(&self.ut_extra()).bytes(&self.uni_extra()).bytes(&self.aes_extra());
        let mut f = self.flags;
        if descriptor { f |= FLAG_DESCRIPTOR; }
        let mut b = Le::default();
        b.u32(SIG_LOCAL).u16(self.version_needed()).u16(f).u16(self.method).u16(dt).u16(dd);
        if descriptor { b.u32(0).u32(if self.local_zip64 { U32_MAX as u32 } else { 0 }).u32(if self.local_zip64 { U32_MAX as u32 } else { 0 }); }
        else if self.local_zip64 { b.u32(self.crc_field()).u32(U32_MAX as u32).u32(U32_MAX as u32); }
        else { b.u32(self.crc_field()).u32(self.csize as u32).u32(self.usize as u32); }
        b.u16(self.name.len() as u16).u16(extra.0.len() as u16).bytes(&self.name).bytes(&extra.0);
        b.0
    }

    fn central(&self, disk: u32, off: u64, descriptor: bool) -> Vec<u8> {
        let (dt, dd, _) = dos_datetime(self.mtime);
        let (big_u, big_c, big_o) = (self.usize >= U32_MAX, self.csize >= U32_MAX, off >= U32_MAX);
        let mut z = Le::default();
        if big_u || big_c || big_o {
            let mut body = Le::default();
            if big_u { body.u64(self.usize); }
            if big_c { body.u64(self.csize); }
            if big_o { body.u64(off); }
            z.u16(EXTRA_ZIP64).u16(body.0.len() as u16).bytes(&body.0);
        }
        z.bytes(&self.ut_extra()).bytes(&self.uni_extra()).bytes(&self.aes_extra());
        let ver = if big_u || big_c || big_o { self.version_needed().max(45) } else { self.version_needed() };
        let mut f = self.flags;
        if descriptor { f |= FLAG_DESCRIPTOR; }
        let mut b = Le::default();
        b.u32(SIG_CENTRAL).u16((3 << 8) | 63).u16(ver).u16(f).u16(self.method).u16(dt).u16(dd)
            .u32(self.crc_field())
            .u32(if big_c { U32_MAX as u32 } else { self.csize as u32 })
            .u32(if big_u { U32_MAX as u32 } else { self.usize as u32 })
            .u16(self.name.len() as u16).u16(z.0.len() as u16).u16(0).u16(disk.min(0xFFFF) as u16).u16(0)
            .u32(self.ext_attr())
            .u32(if big_o { U32_MAX as u32 } else { off as u32 })
            .bytes(&self.name).bytes(&z.0);
        b.0
    }
}

struct CdRec { hdr: Hdr, disk: u32, off: u64, descriptor: bool }

// ---------------------------------------------------------------- codecs

thread_local! {
    static DEFL: RefCell<Option<(i32, libdeflater::Compressor)>> = const { RefCell::new(None) };
    static ZSTDC: RefCell<Option<(i32, zstd::bulk::Compressor<'static>)>> = const { RefCell::new(None) };
}

fn deflate_level(l: Level) -> i32 { match l { Level::Store => 0, Level::Fast => 3, Level::Normal => 6, Level::Maximum => 9 } }
fn zstd_level(l: Level) -> i32 { match l { Level::Store => 0, Level::Fast => 3, Level::Normal => 9, Level::Maximum => 19 } }
fn bzip2_level(l: Level) -> u32 { match l { Level::Store | Level::Fast => 1, Level::Normal => 6, Level::Maximum => 9 } }

/// Whole-buffer compression with per-thread codec-context reuse.
fn compress_buf(m: Method, l: Level, data: &[u8]) -> Result<Vec<u8>> {
    match m {
        Method::Store => Ok(data.to_vec()),
        Method::Deflate => DEFL.with(|c| {
            let lvl = deflate_level(l);
            let mut c = c.borrow_mut();
            if c.as_ref().map(|x| x.0) != Some(lvl) {
                *c = Some((lvl, libdeflater::Compressor::new(libdeflater::CompressionLvl::new(lvl).unwrap())));
            }
            let comp = &mut c.as_mut().unwrap().1;
            let mut out = vec![0u8; comp.deflate_compress_bound(data.len())];
            let n = comp.deflate_compress(data, &mut out).map_err(|e| Error::new(ErrorKind::Io, format!("deflate: {e:?}")))?;
            out.truncate(n);
            Ok(out)
        }),
        Method::Zstd => ZSTDC.with(|c| {
            let lvl = zstd_level(l);
            let mut c = c.borrow_mut();
            if c.as_ref().map(|x| x.0) != Some(lvl) { *c = Some((lvl, zstd::bulk::Compressor::new(lvl)?)); }
            Ok(c.as_mut().unwrap().1.compress(data)?)
        }),
        Method::Ppmd => {
            let (order, mb) = ppmd_params(l);
            let mut v = ppmd_header(order, mb).to_vec();
            let mut e = ppmd_rust::Ppmd8Encoder::new(Vec::new(), order, mb << 20, ppmd_rust::RestoreMethod::Restart).map_err(|er| Error::new(ErrorKind::Io, format!("{er:?}")))?;
            e.write_all(data)?;
            v.extend_from_slice(&e.finish(true)?);
            Ok(v)
        }
        Method::Lzma => {
            let mut e = lzma_zip_encoder(l)?;
            e.write_all(data)?;
            let mut v = e.finish()?;
            fix_lzma_header(&mut v);
            Ok(v)
        }
        Method::Bzip2 => {
            let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(bzip2_level(l)));
            e.write_all(data)?;
            Ok(e.finish()?)
        }
    }
}

fn lzma_zip_encoder(l: Level) -> Result<xz2::write::XzEncoder<Vec<u8>>> {
    let preset = match l { Level::Store | Level::Fast => 1, Level::Normal => 5, Level::Maximum => 9 };
    let o = xz2::stream::LzmaOptions::new_preset(preset).map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
    let st = xz2::stream::Stream::new_lzma_encoder(&o).map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
    Ok(xz2::write::XzEncoder::new_stream(Vec::new(), st))
}
/// Turn a `.lzma` (alone) header [props(5) size(8)] into the ZIP LZMA header [ver(2) psz(2) props(5)].
fn fix_lzma_header(v: &mut Vec<u8>) {
    if v.len() < 13 { return; }
    let mut h = vec![9u8, 20, 5, 0];
    h.extend_from_slice(&v[..5]);
    v.splice(..13, h);
}

fn ppmd_params(l: Level) -> (u32, u32) { match l { Level::Store | Level::Fast => (4, 4), Level::Normal => (6, 24), Level::Maximum => (8, 192) } }
fn ppmd_header(order: u32, mb: u32) -> [u8; 2] { (((order - 1) & 0xF) as u16 | ((((mb - 1) & 0xFF) as u16) << 4)).to_le_bytes() }

fn method_code(m: Method) -> u16 { match m { Method::Store => M_STORE, Method::Deflate => M_DEFLATE, Method::Zstd => M_ZSTD, Method::Bzip2 => M_BZIP2, Method::Lzma => M_LZMA, Method::Ppmd => M_PPMD } }

fn deflate_block(data: &[u8], level: u32) -> Vec<u8> {
    use flate2::{Compress, Compression, FlushCompress};
    let mut c = Compress::new(Compression::new(level), false);
    let mut out = Vec::with_capacity(data.len() / 2 + 4096);
    let mut input = data;
    loop {
        let before = c.total_in();
        if out.capacity() - out.len() < 64 { out.reserve(out.capacity() / 2 + 4096); }
        c.compress_vec(input, &mut out, FlushCompress::Full).expect("deflate");
        input = &input[(c.total_in() - before) as usize..];
        if input.is_empty() && out.len() < out.capacity() { break; }
    }
    out
}

// ---------------------------------------------------------------- encryption state

enum Enc { None, Zip(ZipCrypto), Aes(AesCtx) }

impl Enc {
    fn new(e: &Encryption) -> Enc {
        match e {
            Encryption::None => Enc::None,
            Encryption::ZipCrypto(p) => Enc::Zip(ZipCrypto::new(p.as_bytes())),
            Encryption::Aes { strength, password } => Enc::Aes(AesCtx::new_encrypt(password.as_bytes(), *strength)),
        }
    }
    /// Bytes preceding the ciphertext. `check` is the ZipCrypto verification byte.
    fn prefix(&mut self, check: u8) -> Vec<u8> {
        match self {
            Enc::None => vec![],
            Enc::Zip(z) => z.header(check).to_vec(),
            Enc::Aes(a) => { let mut v = a.salt.clone(); v.extend_from_slice(&a.verifier); v }
        }
    }
    fn process(&mut self, buf: &mut [u8]) {
        match self { Enc::None => {}, Enc::Zip(z) => z.encrypt(buf), Enc::Aes(a) => a.encrypt(buf) }
    }
    fn trailer(self) -> Vec<u8> { match self { Enc::Aes(a) => a.finish().to_vec(), _ => vec![] } }
}

fn aes_strength_of(e: &Encryption) -> Option<u8> { match e { Encryption::Aes { strength, .. } => Some(*strength), _ => None } }

// ---------------------------------------------------------------- messages

struct Meta { name: Vec<u8>, efs: bool, unicode_extra: bool, mtime: i64, mode: u32, is_dir: bool }

struct Packed { data: Vec<u8>, crc: u32, usize: u64, method: u16, real_method: u16, flags: u16, stored_by_skip: bool }

enum Msg {
    Whole(Meta, Packed),
    LargeBegin { meta: Meta, hdr_method: u16, real_method: u16, flags: u16, zip64: bool, prefix: Vec<u8> },
    Chunk(Vec<u8>),
    LargeEnd { crc: u32, usize: u64, trailer: Vec<u8> },
}

fn unix_mode(d: &EntryDesc) -> u32 {
    let perm = d.mode & 0o7777;
    match d.kind {
        EntryKind::Dir => 0o040000 | if perm == 0 { 0o755 } else { perm },
        EntryKind::File => 0o100000 | if perm == 0 { 0o644 } else { perm },
        EntryKind::Symlink => 0o120777,
    }
}

fn make_meta(d: &EntryDesc, opts: &ZipOptions) -> Result<Meta> {
    let is_dir = d.kind == EntryKind::Dir;
    let mut name = d.name.clone();
    if is_dir && !name.ends_with('/') { name.push('/'); }
    let (bytes, efs, uni) = match &opts.legacy_codepage {
        None => (name.as_bytes().to_vec(), true, !name.is_ascii()),
        Some(cp) => (
            codepage::encode(cp, &name).ok_or_else(|| Error::new(ErrorKind::InvalidInput,
                format!("name '{name}' cannot be represented in code page {cp}; switch to UTF-8 mode")))?,
            false, false),
    };
    if bytes.len() > 0xFFFF { return Err(Error::new(ErrorKind::PathTooLong, format!("name too long: {name}"))); }
    Ok(Meta { name: bytes, efs, unicode_extra: uni, mtime: unix_secs(d.mtime), mode: unix_mode(d), is_dir })
}

fn should_store(d: &EntryDesc, opts: &ZipOptions) -> bool {
    opts.level == Level::Store || opts.method == Method::Store || d.size == 0
        || (opts.high_speed != HighSpeed::Off && opts.skip.should_store(&d.name))
}

/// Compress one small entry fully in memory (runs on the codec pool).
fn pack_small(d: &EntryDesc, opts: &ZipOptions, progress: &Progress, cancel: &CancelToken) -> Result<(Meta, Packed)> {
    cancel.check()?;
    let meta = make_meta(d, opts)?;
    progress.set_current(&d.name);
    let raw: Vec<u8> = match d.kind {
        EntryKind::Dir => vec![],
        EntryKind::Symlink => d.link_target.clone().unwrap_or_default().into_bytes(),
        EntryKind::File => {
            let mut v = Vec::with_capacity(d.size as usize);
            File::open(&d.abs)?.read_to_end(&mut v)?;
            progress.add_read(v.len() as u64);
            v
        }
    };
    let crc = crc32fast::hash(&raw);
    let usize_ = raw.len() as u64;
    let by_skip = d.kind == EntryKind::File && opts.method != Method::Store && opts.level != Level::Store
        && opts.high_speed != HighSpeed::Off && opts.skip.should_store(&d.name);
    let (mut payload, mut real) = if meta.is_dir || d.kind == EntryKind::Symlink || should_store(d, opts) {
        (raw, M_STORE)
    } else {
        let c = compress_buf(opts.method, opts.level, &raw)?;
        if c.len() >= raw.len() { (raw, M_STORE) } else { (c, method_code(opts.method)) }
    };
    let mut flags = if meta.efs { FLAG_EFS } else { 0 };
    if real == M_LZMA { flags |= 2; } // EOS marker present
    let mut method = real;
    if !meta.is_dir {
        let mut enc = Enc::new(&opts.encryption);
        if !matches!(enc, Enc::None) {
            let mut out = enc.prefix((crc >> 24) as u8);
            let start = out.len();
            out.extend_from_slice(&payload);
            enc.process(&mut out[start..]);
            out.extend_from_slice(&enc.trailer_ref());
            payload = out;
            flags |= FLAG_ENCRYPTED;
            if aes_strength_of(&opts.encryption).is_some() { method = M_AES; }
        }
    }
    let _ = &mut real;
    Ok((meta, Packed { data: payload, crc, usize: usize_, method, real_method: real, flags, stored_by_skip: by_skip }))
}

impl Enc {
    fn trailer_ref(self) -> Vec<u8> { self.trailer() }
}

// ---------------------------------------------------------------- public API

pub fn create_zip(out: &Path, entries: &[EntryDesc], opts: &ZipOptions, cancel: &CancelToken, progress: &Progress) -> Result<ZipStats> {
    if let Some(s) = opts.split_size {
        if s < MIN_SPLIT { return Err(Error::new(ErrorKind::InvalidSplitSize, "minimum volume size is 100 KB")); }
        if s >= (4u64 << 30) { return Err(Error::new(ErrorKind::FourGbLimit, "ZIP volumes must be smaller than 4 GB; use 7Z")); }
    }
    if let Encryption::Aes { strength, .. } = &opts.encryption {
        if !(1..=3).contains(strength) { return Err(Error::new(ErrorKind::InvalidInput, "AES strength must be 1..3")); }
    }
    if let Some(parent) = out.parent() { if !parent.as_os_str().is_empty() { std::fs::create_dir_all(parent)?; } }
    progress.set_totals(entries.len() as u64, entries.iter().map(|e| e.size).sum());
    progress.set_phase("compressing");

    let sink = Sink::open(out, opts.split_size)?;
    let pool = crate::sys::build_pool(opts.threads);
    let (tx, rx) = crossbeam_channel::bounded::<Msg>(4);

    let failed = std::sync::atomic::AtomicBool::new(false);
    let result = std::thread::scope(|s| {
        let failed_ref = &failed;
        let writer = s.spawn(move || writer_thread(sink, rx, opts, progress, cancel, failed_ref));
        let orch = orchestrate(entries, opts, &pool, &tx, cancel, progress);
        if orch.is_err() { failed.store(true, std::sync::atomic::Ordering::SeqCst); }
        drop(tx);
        let w = writer.join().unwrap_or_else(|_| Err(Error::new(ErrorKind::Io, "writer thread panicked")));
        match (orch, w) {
            (Err(e), Err(w)) => Err(if w.kind == ErrorKind::Cancelled { w } else { e }),
            (_, Err(e)) => Err(e),
            (Err(e), _) => Err(e),
            (Ok(si), Ok(mut st)) => { st.stored_by_skiplist = si; Ok(st) }
        }
    });
    result
}

fn orchestrate(entries: &[EntryDesc], opts: &ZipOptions, pool: &rayon::ThreadPool, tx: &crossbeam_channel::Sender<Msg>,
               cancel: &CancelToken, progress: &Progress) -> Result<u64> {
    use rayon::prelude::*;
    let batch_cap = (opts.memory_budget / 8).clamp(8 << 20, 128 << 20);
    let send = |m: Msg| tx.send(m).map_err(|_| Error::new(ErrorKind::Io, "writer stopped"));
    let mut skipped = 0u64;
    let mut i = 0;
    while i < entries.len() {
        cancel.check()?;
        let e = &entries[i];
        if e.kind == EntryKind::File && e.size >= LARGE {
            stream_large(e, opts, pool, &send, cancel, progress)?;
            i += 1;
            continue;
        }
        let (mut j, mut bytes) = (i, 0u64);
        while j < entries.len() && !(entries[j].kind == EntryKind::File && entries[j].size >= LARGE) && (bytes < batch_cap || j == i) {
            bytes += entries[j].size.max(1);
            j += 1;
        }
        let batch = &entries[i..j];
        let packed: Vec<Result<(Meta, Packed)>> = pool.install(|| batch.par_iter().map(|d| pack_small(d, opts, progress, cancel)).collect());
        for p in packed {
            let (m, pk) = p?;
            if pk.stored_by_skip { skipped += 1; }
            send(Msg::Whole(m, pk))?;
        }
        i = j;
    }
    Ok(skipped)
}

fn stream_large(d: &EntryDesc, opts: &ZipOptions, pool: &rayon::ThreadPool, send: &dyn Fn(Msg) -> Result<()>,
                cancel: &CancelToken, progress: &Progress) -> Result<()> {
    use rayon::prelude::*;
    let meta = make_meta(d, opts)?;
    progress.set_current(&d.name);
    let store = should_store(d, opts);
    let code = if store { M_STORE } else { method_code(opts.method) };
    let mut enc = Enc::new(&opts.encryption);
    let encrypted = !matches!(enc, Enc::None);
    let mut flags = if meta.efs { FLAG_EFS } else { 0 };
    if code == M_LZMA { flags |= 2; }
    let (dt, _, _) = dos_datetime(meta.mtime);
    let prefix = enc.prefix((dt >> 8) as u8);
    let mut hdr_method = code;
    if encrypted {
        flags |= FLAG_ENCRYPTED;
        if aes_strength_of(&opts.encryption).is_some() { hdr_method = M_AES; }
    }
    send(Msg::LargeBegin { meta, hdr_method, real_method: code, flags, zip64: d.size >= ZIP64_HINT, prefix })?;

    let mut f = File::open(&d.abs)?;
    let mut crc = crc32fast::Hasher::new();
    let mut total = 0u64;
    let win_blocks = if store { pool.current_num_threads().min(4) } else { (pool.current_num_threads() * 2).clamp(2, 64).min(((opts.memory_budget / 8) as usize / BLOCK).max(2)) };
    let mut buf = vec![0u8; BLOCK * win_blocks];

    // Sequential streaming codecs (zstd multithread internally / bzip2).
    enum Seq { Zstd(zstd::stream::write::Encoder<'static, Vec<u8>>), Bz(bzip2::write::BzEncoder<Vec<u8>>), Lz(xz2::write::XzEncoder<Vec<u8>>, bool), Pp(ppmd_rust::Ppmd8Encoder<Vec<u8>>) }
    let mut seq = if store { None } else {
        match opts.method {
            Method::Zstd => {
                let mut z = zstd::stream::write::Encoder::new(Vec::new(), zstd_level(opts.level))?;
                let n = if opts.threads == 0 { crate::sys::perf_cores() } else { opts.threads };
                let _ = z.multithread(n as u32);
                Some(Seq::Zstd(z))
            }
            Method::Ppmd => {
                let (order, mb) = ppmd_params(opts.level);
                let e = ppmd_rust::Ppmd8Encoder::new(Vec::new(), order, mb << 20, ppmd_rust::RestoreMethod::Restart).map_err(|er| Error::new(ErrorKind::Io, format!("{er:?}")))?;
                let mut hv = ppmd_header(order, mb).to_vec();
                enc.process(&mut hv);
                send(Msg::Chunk(hv))?;
                Some(Seq::Pp(e))
            }
            Method::Lzma => Some(Seq::Lz(lzma_zip_encoder(opts.level)?, false)),
            Method::Bzip2 => Some(Seq::Bz(bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(bzip2_level(opts.level))))),
            _ => None,
        }
    };

    let emit = |mut v: Vec<u8>, enc: &mut Enc| -> Result<()> { enc.process(&mut v); send(Msg::Chunk(v)) };

    loop {
        cancel.check()?;
        let mut n = 0;
        while n < buf.len() {
            let r = f.read(&mut buf[n..])?;
            if r == 0 { break; }
            n += r;
        }
        if n == 0 { break; }
        progress.add_read(n as u64);
        total += n as u64;
        let data = &buf[..n];
        let blocks: Vec<&[u8]> = data.chunks(BLOCK).collect();
        match (&mut seq, store, opts.method) {
            (_, true, _) | (None, _, Method::Store) => {
                let crcs: Vec<crc32fast::Hasher> = pool.install(|| blocks.par_iter().map(|b| { let mut h = crc32fast::Hasher::new(); h.update(b); h }).collect());
                for h in &crcs { crc.combine(h); }
                emit(data.to_vec(), &mut enc)?;
            }
            (None, _, _) => {
                let lvl = deflate_level(opts.level) as u32;
                let out: Vec<(Vec<u8>, crc32fast::Hasher)> = pool.install(|| blocks.par_iter().map(|b| {
                    let mut h = crc32fast::Hasher::new(); h.update(b); (deflate_block(b, lvl), h)
                }).collect());
                for (v, h) in out { crc.combine(&h); emit(v, &mut enc)?; }
            }
            (Some(Seq::Zstd(z)), _, _) => {
                crc.update(data);
                z.write_all(data)?;
                let v = std::mem::take(z.get_mut());
                if !v.is_empty() { emit(v, &mut enc)?; }
            }
            (Some(Seq::Lz(z, started)), _, _) => {
                crc.update(data);
                z.write_all(data)?;
                if !*started && z.get_ref().len() >= 13 {
                    let mut v = std::mem::take(z.get_mut());
                    fix_lzma_header(&mut v);
                    *started = true;
                    emit(v, &mut enc)?;
                } else if *started {
                    let v = std::mem::take(z.get_mut());
                    if !v.is_empty() { emit(v, &mut enc)?; }
                }
            }
            (Some(Seq::Pp(z)), _, _) => {
                crc.update(data);
                z.write_all(data)?;
                let v = std::mem::take(z.get_mut());
                if !v.is_empty() { emit(v, &mut enc)?; }
            }
            (Some(Seq::Bz(z)), _, _) => {
                crc.update(data);
                z.write_all(data)?;
                let v = std::mem::take(z.get_mut());
                if !v.is_empty() { emit(v, &mut enc)?; }
            }
        }
        if n < buf.len() { break; }
    }
    // finish codec
    match seq {
        Some(Seq::Zstd(z)) => emit(z.finish()?, &mut enc)?,
        Some(Seq::Bz(z)) => emit(z.finish()?, &mut enc)?,
        Some(Seq::Pp(z)) => emit(z.finish(true)?, &mut enc)?,
        Some(Seq::Lz(z, started)) => { let mut v = z.finish()?; if !started { fix_lzma_header(&mut v); } emit(v, &mut enc)? }
        None if !store && opts.method == Method::Deflate => emit(vec![0x03, 0x00], &mut enc)?,
        None => {}
    }
    send(Msg::LargeEnd { crc: crc.finalize(), usize: total, trailer: enc.trailer() })
}

struct Large { hdr: Hdr, disk: u32, off: u64, start_total: u64 }

fn writer_thread(mut sink: Sink, rx: crossbeam_channel::Receiver<Msg>, opts: &ZipOptions, progress: &Progress,
                 cancel: &CancelToken, failed: &std::sync::atomic::AtomicBool) -> Result<ZipStats> {
    let r = writer_loop(&mut sink, rx, opts, progress, cancel).and_then(|v| {
        if failed.load(std::sync::atomic::Ordering::SeqCst) { Err(Error::new(ErrorKind::Io, "aborted: producer failed")) } else { Ok(v) }
    });
    match r {
        Ok((cd, stats)) => finalize(sink, cd, opts, stats),
        Err(e) => { sink.abort(); Err(e) }
    }
}

fn writer_loop(sink: &mut Sink, rx: crossbeam_channel::Receiver<Msg>, opts: &ZipOptions, progress: &Progress,
               cancel: &CancelToken) -> Result<(Vec<CdRec>, ZipStats)> {
    let mut cd: Vec<CdRec> = Vec::new();
    let mut stats = ZipStats::default();
    let mut cur: Option<Large> = None;
    let aes = aes_strength_of(&opts.encryption);
    while let Ok(m) = rx.recv() {
        cancel.check()?;
        match m {
            Msg::Whole(meta, pk) => {
                let hdr = Hdr {
                    name: meta.name, unicode_extra: meta.unicode_extra, method: pk.method, real_method: pk.real_method,
                    flags: pk.flags, mtime: meta.mtime, mode: meta.mode, is_dir: meta.is_dir, crc: pk.crc, csize: pk.data.len() as u64,
                    usize: pk.usize, aes_strength: if pk.method == M_AES { aes } else { None }, local_zip64: false,
                };
                let local = hdr.local(false);
                sink.ensure_room(local.len() as u64)?;
                let (disk, off) = sink.at();
                sink.write(&local)?;
                sink.write(&pk.data)?;
                progress.add_written((local.len() + pk.data.len()) as u64);
                progress.file_done();
                stats.entries += 1;
                stats.bytes_in += pk.usize;
                cd.push(CdRec { hdr, disk, off, descriptor: false });
            }
            Msg::LargeBegin { meta, hdr_method, real_method, flags, zip64, prefix } => {
                let hdr = Hdr {
                    name: meta.name, unicode_extra: meta.unicode_extra, method: hdr_method, real_method, flags,
                    mtime: meta.mtime, mode: meta.mode, is_dir: false, crc: 0, csize: 0, usize: 0,
                    aes_strength: if hdr_method == M_AES { aes } else { None }, local_zip64: zip64,
                };
                let local = hdr.local(true);
                sink.ensure_room(local.len() as u64)?;
                let (disk, off) = sink.at();
                let start_total = sink.total;
                sink.write(&local)?;
                sink.write(&prefix)?;
                cur = Some(Large { hdr, disk, off, start_total });
            }
            Msg::Chunk(v) => {
                sink.write(&v)?;
                progress.add_written(v.len() as u64);
            }
            Msg::LargeEnd { crc, usize, trailer } => {
                let mut l = cur.take().ok_or_else(|| Error::new(ErrorKind::Io, "protocol error"))?;
                sink.write(&trailer)?;
                let header_len = l.hdr.local(true).len() as u64;
                l.hdr.csize = sink.total - l.start_total - header_len;
                l.hdr.usize = usize;
                l.hdr.crc = crc;
                let z64 = l.hdr.local_zip64 || l.hdr.csize >= U32_MAX || usize >= U32_MAX;
                let mut d = Le::default();
                d.u32(SIG_DESC).u32(l.hdr.crc_field());
                if z64 { d.u64(l.hdr.csize).u64(usize); } else { d.u32(l.hdr.csize as u32).u32(usize as u32); }
                sink.write(&d.0)?;
                progress.file_done();
                stats.entries += 1;
                stats.bytes_in += usize;
                cd.push(CdRec { hdr: l.hdr, disk: l.disk, off: l.off, descriptor: true });
            }
        }
    }
    cancel.check()?;
    Ok((cd, stats))
}

fn finalize(mut sink: Sink, cd: Vec<CdRec>, opts: &ZipOptions, mut stats: ZipStats) -> Result<ZipStats> {
    let r = (|| -> Result<()> {
        let (cd_disk, cd_off) = { sink.ensure_room(1)?; sink.at() };
        let mut cd_size = 0u64;
        let mut per_disk = 0u64;
        let mut first = true;
        let (mut cd_disk, mut cd_off) = (cd_disk, cd_off);
        for r in &cd {
            let b = r.hdr.central(r.disk, r.off, r.descriptor);
            let before = sink.disk;
            sink.ensure_room(b.len() as u64)?;
            if first { cd_disk = sink.disk; cd_off = sink.pos; first = false; }
            if sink.disk != before { per_disk = 0; }
            sink.write(&b)?;
            cd_size += b.len() as u64;
            per_disk += 1;
        }
        let n = cd.len() as u64;
        let comment = opts.comment.as_bytes();
        let comment = &comment[..comment.len().min(0xFFFF)];
        let need64 = n >= 0xFFFF || cd_off >= U32_MAX || cd_size >= U32_MAX || sink.disk >= 0xFFFF;
        if need64 {
            sink.ensure_room(56 + 20 + 22 + comment.len() as u64)?;
            let (z_disk, z_off) = sink.at();
            let mut z = Le::default();
            z.u32(SIG_EOCD64).u64(44).u16(45).u16(45).u32(sink.disk).u32(cd_disk).u64(per_disk).u64(n).u64(cd_size).u64(cd_off);
            z.u32(SIG_LOC64).u32(z_disk).u64(z_off).u32(sink.disk + 1);
            sink.write(&z.0)?;
        } else { sink.ensure_room(22 + comment.len() as u64)?; }
        let mut e = Le::default();
        let c16 = |v: u64| v.min(0xFFFF) as u16;
        e.u32(SIG_EOCD).u16(c16(sink.disk as u64)).u16(c16(cd_disk as u64)).u16(if need64 { 0xFFFF } else { c16(per_disk) })
            .u16(if need64 { 0xFFFF } else { n as u16 })
            .u32(if need64 { U32_MAX as u32 } else { cd_size as u32 }).u32(if need64 { U32_MAX as u32 } else { cd_off as u32 })
            .u16(comment.len() as u16).bytes(comment);
        sink.write(&e.0)?;
        Ok(())
    })();
    if let Err(e) = r { sink.abort(); return Err(e); }
    stats.bytes_out = sink.total;
    match sink.finish() {
        Ok(v) => { stats.volumes = v; Ok(stats) }
        Err(e) => Err(e),
    }
}
