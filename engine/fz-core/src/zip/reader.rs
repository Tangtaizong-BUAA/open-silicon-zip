//! ZIP reader: central-directory parsing (ZIP64, spanned, SFX), code-page intelligence,
//! streaming decode with encryption and CRC verification.
use super::crypto::{self, AesCtx, ZipCrypto};
use super::format::*;
use crate::codepage;
use crate::error::{Error, ErrorKind, Result};
use crate::pathsafe::normalize_read_name;
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncKind { None, ZipCrypto, Aes(u8) }

#[derive(Debug, Clone)]
pub struct ZipEntry {
    pub index: usize,
    pub raw_name: Vec<u8>,
    pub name: String,
    pub flags: u16,
    /// Real compression method (AES wrapper resolved).
    pub method: u16,
    pub csize: u64,
    pub size: u64,
    pub crc: u32,
    pub mtime: i64,
    pub disk: u32,
    pub local_off: u64,
    pub mode: u32,
    pub ext_attr: u32,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub enc: EncKind,
    pub aes_ae2: bool,
    pub efs_or_7075: bool,
    pub dos_time: u16,
}

#[derive(Debug, Clone)]
pub struct ReadOptions {
    /// None = auto-detect.
    pub codepage: Option<String>,
    pub default_codepage: String,
    /// Treat flag-less names that are valid UTF-8 as UTF-8 ("UNIX zip", FR-804).
    pub unix_utf8: bool,
}
impl Default for ReadOptions {
    fn default() -> Self { ReadOptions { codepage: None, default_codepage: "ibm437".into(), unix_utf8: true } }
}

pub(crate) struct Vols { files: Vec<File>, sizes: Vec<u64>, pub paths: Vec<PathBuf>, base: u64 }

impl Vols {
    /// Read at (disk, off), continuing into following volumes at EOF of a disk.
    pub fn read_at(&self, mut disk: u32, mut off: u64, buf: &mut [u8]) -> io::Result<usize> {
        let mut done = 0;
        while done < buf.len() {
            let d = disk as usize;
            if d >= self.files.len() { break; }
            let size = self.sizes[d];
            let o = off + if d == 0 { self.base } else { 0 };
            if o >= size { disk += 1; off = 0; continue; }
            let n = ((size - o) as usize).min(buf.len() - done);
            self.files[d].read_exact_at(&mut buf[done..done + n], o)?;
            done += n; off += n as u64;
        }
        Ok(done)
    }
    pub fn read_exact(&self, disk: u32, off: u64, buf: &mut [u8]) -> Result<()> {
        if self.read_at(disk, off, buf)? < buf.len() { return Err(Error::new(ErrorKind::BrokenArchive, "unexpected end of archive")); }
        Ok(())
    }
    fn advance(&self, mut disk: u32, mut off: u64, mut n: u64) -> (u32, u64) {
        loop {
            let d = disk as usize;
            if d >= self.sizes.len() { return (disk, off + n); }
            let size = self.sizes[d] - if d == 0 { self.base } else { 0 };
            if off + n < size || d + 1 == self.sizes.len() { return (disk, off + n); }
            n -= size - off; disk += 1; off = 0;
        }
    }
}

struct RawRec { raw_name: Vec<u8>, entry: ZipEntry, uni: Option<Vec<u8>>, range: (usize, usize) }

pub struct ZipArchive {
    pub(crate) vols: Vols,
    pub entries: Vec<ZipEntry>,
    raw: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    pub comment: Vec<u8>,
    pub codepage_used: String,
    pub spanned: bool,
    opts: ReadOptions,
    pub(crate) cd: Vec<u8>,
    pub(crate) cd_ranges: Vec<(usize, usize)>,
    pub(crate) cd_start: u64,
    pub(crate) base: u64,
}

fn split_volume_paths(p: &Path) -> Result<(Vec<PathBuf>, bool)> {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let is_z = ext.len() == 3 && ext.starts_with('z') && ext[1..].chars().all(|c| c.is_ascii_digit());
    let stem = p.with_extension("");
    let mut zip_path = p.to_path_buf();
    if is_z {
        zip_path = ["zip", "ZIP"].iter().map(|e| stem.with_extension(e)).find(|c| c.exists())
            .ok_or_else(|| Error::new(ErrorKind::MissingVolume, format!("missing final volume {}.zip", stem.display())))?;
    }
    let mut vols = Vec::new();
    for i in 1..100000u32 {
        let mut s = stem.clone().into_os_string();
        s.push(format!(".z{:02}", i));
        let c = PathBuf::from(s);
        if c.exists() { vols.push(c); } else {
            if is_z || zip_has_spanned_marker(&zip_path) {
                // contiguous set required: anything numbered above must not exist
                let mut gap = false;
                for j in i + 1..i + 4 { let mut s = stem.clone().into_os_string(); s.push(format!(".z{:02}", j)); if PathBuf::from(s).exists() { gap = true; } }
                if gap { return Err(Error::new(ErrorKind::MissingVolume, format!("missing volume {}.z{:02}", stem.display(), i))); }
            }
            break;
        }
    }
    let spanned = !vols.is_empty();
    vols.push(zip_path);
    Ok((vols, spanned))
}
fn zip_has_spanned_marker(_p: &Path) -> bool { false }

impl ZipArchive {
    pub fn open(path: &Path, opts: ReadOptions) -> Result<ZipArchive> {
        let (paths, spanned) = split_volume_paths(path)?;
        let mut files = Vec::new();
        let mut sizes = Vec::new();
        for p in &paths {
            let f = File::open(p)?;
            sizes.push(f.metadata()?.len());
            files.push(f);
        }
        let last = files.len() - 1;
        let (last_f, last_size) = (&files[last], sizes[last]);
        // locate EOCD (scan last 64K+22 bytes)
        let tail_len = last_size.min(0x10000 + 22 + 20 + 56) as usize;
        let mut tail = vec![0u8; tail_len];
        last_f.read_exact_at(&mut tail, last_size - tail_len as u64)?;
        let mut pos = None;
        if tail.len() >= 22 {
            for i in (0..=tail.len() - 22).rev() {
                if rd32(&tail, i) == SIG_EOCD {
                    let cl = rd16(&tail, i + 20) as usize;
                    if i + 22 + cl <= tail.len() { pos = Some(i); break; }
                }
            }
        }
        let i = pos.ok_or_else(|| Error::new(ErrorKind::UnsupportedFormat, "not a ZIP archive (no end-of-central-directory)"))?;
        let eocd_abs = last_size - tail_len as u64 + i as u64;
        let this_disk = rd16(&tail, i + 4) as u32;
        let mut cd_disk = rd16(&tail, i + 6) as u32;
        let mut entries_total = rd16(&tail, i + 10) as u64;
        let mut cd_size = rd32(&tail, i + 12) as u64;
        let mut cd_off = rd32(&tail, i + 16) as u64;
        let comment = tail[i + 22..i + 22 + rd16(&tail, i + 20) as usize].to_vec();
        let mut eocd64_pos: Option<u64> = None;
        if i >= 20 && rd32(&tail, i - 20) == SIG_LOC64 {
            let off64 = rd64(&tail, i - 20 + 8);
            let mut z = [0u8; 56];
            if !spanned && last_f.read_exact_at(&mut z, off64).is_ok() && rd32(&z, 0) == SIG_EOCD64 {
                entries_total = rd64(&z, 32); cd_size = rd64(&z, 40); cd_off = rd64(&z, 48); cd_disk = rd32(&z, 16);
                eocd64_pos = Some(off64);
            } else if spanned {
                let mut z = [0u8; 56];
                let d = rd32(&tail, i - 20 + 4);
                if files.get(d as usize).map(|f| f.read_exact_at(&mut z, off64).is_ok()).unwrap_or(false) && rd32(&z, 0) == SIG_EOCD64 {
                    entries_total = rd64(&z, 32); cd_size = rd64(&z, 40); cd_off = rd64(&z, 48); cd_disk = rd32(&z, 20);
                    eocd64_pos = Some(off64);
                }
            }
        }
        if spanned && this_disk as usize != last {
            return Err(Error::new(ErrorKind::MissingVolume, format!("archive spans {} volumes but only {} found", this_disk + 1, last + 1)));
        }
        // SFX / prepended data: shift = actual - recorded position of central directory
        let mut base = 0u64;
        if !spanned {
            let end_of_cd = eocd64_pos.unwrap_or(eocd_abs);
            let recorded_end = cd_off.checked_add(cd_size).ok_or_else(|| Error::new(ErrorKind::BrokenArchive, "central directory offset overflow"))?;
            if recorded_end < end_of_cd { base = end_of_cd - recorded_end; }
        }
        let vols = Vols { files, sizes, paths, base };
        if !spanned { cd_disk = 0; }
        let available: u64 = vols.sizes.iter().sum();
        if cd_size > available || cd_size > crate::sys::default_memory_budget().min(256 << 20) {
            return Err(Error::new(ErrorKind::BrokenArchive, "central directory implausibly large"));
        }
        let limits = crate::limits::ListingLimits::default();
        if entries_total > limits.entries as u64 {
            return Err(crate::limits::exceeded("ZIP entry count"));
        }
        let mut budget = crate::limits::ListingBudget::new(limits);
        let mut cd = vec![0u8; cd_size as usize];
        vols.read_exact(cd_disk, cd_off, &mut cd)?;
        let mut recs = Vec::new();
        let mut p = 0usize;
        let mut idx = 0usize;
        while p + 46 <= cd.len() && rd32(&cd, p) == SIG_CENTRAL {
            let flags = rd16(&cd, p + 8);
            let method = rd16(&cd, p + 10);
            let (dt, dd) = (rd16(&cd, p + 12), rd16(&cd, p + 14));
            let crc = rd32(&cd, p + 16);
            let (mut csize, mut size) = (rd32(&cd, p + 20) as u64, rd32(&cd, p + 24) as u64);
            let (nl, el, cl) = (rd16(&cd, p + 28) as usize, rd16(&cd, p + 30) as usize, rd16(&cd, p + 32) as usize);
            let mut disk = rd16(&cd, p + 34) as u32;
            let ext_attr = rd32(&cd, p + 38);
            let mut off = rd32(&cd, p + 42) as u64;
            let ver_made = rd16(&cd, p + 4);
            if p + 46 + nl + el + cl > cd.len() { return Err(Error::new(ErrorKind::BrokenArchive, "truncated central directory")); }
            // Inspect lengths before allocating RawRec, raw-name copies, or a
            // Unicode-path copy. Do not trust EOCD's count: it may under-report.
            let raw_slice = &cd[p + 46..p + 46 + nl];
            let extra = &cd[p + 46 + nl..p + 46 + nl + el];
            let raw_crc = crc32fast::hash(raw_slice);
            let unicode_bytes = extra_fields(extra).into_iter().filter(|(id, d)| {
                *id == EXTRA_UNICODE_PATH && d.len() >= 5 && d[0] == 1 && rd32(d, 1) == raw_crc
            }).map(|(_, d)| d.len() - 5).sum::<usize>();
            if nl > limits.path_bytes || unicode_bytes > limits.path_bytes {
                return Err(crate::limits::exceeded("ZIP path bytes"));
            }
            budget.reserve(nl + unicode_bytes)?;
            let raw_name = cd[p + 46..p + 46 + nl].to_vec();
            let mut real_method = method;
            let mut enc = if flags & FLAG_ENCRYPTED != 0 { EncKind::ZipCrypto } else { EncKind::None };
            let mut ae2 = false;
            let mut uni = None;
            let mut mtime = dos_to_unix(dt, dd);
            for (id, d) in extra_fields(extra) {
                match id {
                    EXTRA_ZIP64 => {
                        let mut q = 0;
                        let take = |q: &mut usize| -> Option<u64> { if *q + 8 <= d.len() { let v = rd64(d, *q); *q += 8; Some(v) } else { None } };
                        if size == U32_MAX { if let Some(v) = take(&mut q) { size = v; } }
                        if csize == U32_MAX { if let Some(v) = take(&mut q) { csize = v; } }
                        if off == U32_MAX { if let Some(v) = take(&mut q) { off = v; } }
                        if disk == 0xFFFF && q + 4 <= d.len() { disk = rd32(d, q); }
                    }
                    EXTRA_AES if d.len() >= 7 => {
                        enc = EncKind::Aes(d[4]);
                        ae2 = rd16(d, 0) == 2;
                        real_method = rd16(d, 5);
                    }
                    EXTRA_UT if d.len() >= 5 && d[0] & 1 == 1 => { mtime = rd32(d, 1) as i64; }
                    EXTRA_UNICODE_PATH if d.len() >= 5 && d[0] == 1 && rd32(d, 1) == raw_crc => { uni = Some(d[5..].to_vec()); }
                    _ => {}
                }
            }
            let unix_host = (ver_made >> 8) == 3;
            let is_dir_name = raw_name.last() == Some(&b'/') || raw_name.last() == Some(&b'\\');
            let is_dir = is_dir_name || (ext_attr & 0x10 != 0 && size == 0 && csize <= 12);
            let mut mode = if unix_host && (ext_attr >> 16) != 0 { ext_attr >> 16 } else {
                (if is_dir { 0o040755 } else if ext_attr & 1 != 0 { 0o100444 } else { 0o100644 }) as u32
            };
            if is_dir && mode & 0o170000 != 0o040000 { mode = (mode & 0o7777) | 0o040000; }
            let is_symlink = unix_host && (mode & 0o170000) == 0o120000;
            let entry = ZipEntry {
                index: idx, raw_name: raw_name.clone(), name: String::new(), flags, method: real_method, csize, size, crc, mtime, disk, local_off: off,
                mode, ext_attr, is_dir, is_symlink, enc, aes_ae2: ae2, efs_or_7075: flags & FLAG_EFS != 0 || uni.is_some(), dos_time: dt,
            };
            recs.push(RawRec { raw_name, entry, uni, range: (p, 46 + nl + el + cl) });
            p += 46 + nl + el + cl;
            idx += 1;
        }
        if (recs.len() as u64) < entries_total.min(0xFFFE) && entries_total != recs.len() as u64 && entries_total < 0xFFFF {
            return Err(Error::new(ErrorKind::BrokenArchive, "central directory entry count mismatch"));
        }
        let raw: Vec<_> = recs.iter().map(|r| (r.raw_name.clone(), r.uni.clone())).collect();
        let cd_ranges: Vec<(usize, usize)> = recs.iter().map(|r| r.range).collect();
        let entries = recs.into_iter().map(|r| r.entry).collect();
        let mut a = ZipArchive { vols, entries, raw, comment, codepage_used: String::new(), spanned, opts: opts.clone(), cd, cd_ranges, cd_start: cd_off, base };
        a.decode_names(opts.codepage.as_deref());
        for e in &a.entries { budget.reserve_json(&e.name)?; }
        Ok(a)
    }

    /// Re-render names under a different code page (FR-013/803). `None` = auto-detect.
    pub fn set_codepage(&mut self, id: Option<&str>) { self.decode_names(id) }

    fn decode_names(&mut self, forced: Option<&str>) {
        let flagless: Vec<&[u8]> = self.entries.iter().zip(&self.raw)
            .filter(|(e, (_, u))| !e.efs_or_7075 && u.is_none() && !e.raw_name.is_ascii()).map(|(_, (n, _))| n.as_slice()).collect();
        let page: String = match forced {
            Some(p) => p.to_string(),
            None => if flagless.is_empty() { self.opts.default_codepage.clone() }
                    else { codepage::auto_detect(&flagless, &self.opts.default_codepage, self.opts.unix_utf8).to_string() },
        };
        for (e, (raw, uni)) in self.entries.iter_mut().zip(&self.raw) {
            let s = if let Some(u) = uni { String::from_utf8_lossy(u).into_owned() }
                else if e.flags & FLAG_EFS != 0 { String::from_utf8_lossy(raw).into_owned() }
                else if raw.is_ascii() { String::from_utf8_lossy(raw).into_owned() }
                else { codepage::decode(&page, raw).0 };
            e.name = normalize_read_name(&s);
        }
        self.codepage_used = page;
    }

    /// Cheap password pre-check material for the smallest encrypted entry (FR-703 fast-reject path).
    pub fn verifier(&self) -> Result<PasswordVerifier> {
        let e = self.entries.iter().filter(|e| e.enc != EncKind::None && !e.is_dir).min_by_key(|e| e.csize)
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "archive has no encrypted entries"))?;
        let (disk, off) = self.data_start(e)?;
        match e.enc {
            EncKind::ZipCrypto => {
                let mut h = [0u8; 12];
                self.vols.read_exact(disk, off, &mut h)?;
                let check = if e.flags & FLAG_DESCRIPTOR != 0 { (e.dos_time >> 8) as u8 } else { (e.crc >> 24) as u8 };
                Ok(PasswordVerifier::ZipCrypto { header: h, check, alt: (e.crc >> 24) as u8, entry: e.index })
            }
            EncKind::Aes(strength) => {
                let sl = crypto::aes_salt_len(strength);
                let mut sv = vec![0u8; sl + 2];
                self.vols.read_exact(disk, off, &mut sv)?;
                Ok(PasswordVerifier::Aes { strength, salt: sv[..sl].to_vec(), verifier: [sv[sl], sv[sl + 1]], entry: e.index })
            }
            EncKind::None => unreachable!(),
        }
    }

    /// Full confirmation: decode the verifier's entry with `pw` (CRC/MAC checked).
    pub fn confirm_password(&self, v: &PasswordVerifier, pw: &str) -> bool {
        self.read_entry(&self.entries[v.entry()], Some(pw), &mut io::sink()).is_ok()
    }

    /// Does any entry require a password?
    pub fn needs_password(&self) -> bool { self.entries.iter().any(|e| e.enc != EncKind::None) }

    /// Locate the compressed-data start for an entry.
    fn data_start(&self, e: &ZipEntry) -> Result<(u32, u64)> {
        let mut h = [0u8; 30];
        self.vols.read_exact(e.disk, e.local_off, &mut h)?;
        if rd32(&h, 0) != SIG_LOCAL { return Err(Error::new(ErrorKind::BrokenArchive, format!("bad local header for {}", e.name))); }
        let n = rd16(&h, 26) as u64 + rd16(&h, 28) as u64;
        Ok(self.vols.advance(e.disk, e.local_off, 30 + n))
    }

    /// Stream-decode an entry into `out`, verifying CRC (and AES MAC). Returns bytes written.
    pub fn read_entry(&self, e: &ZipEntry, password: Option<&str>, out: &mut dyn io::Write) -> Result<u64> {
        if e.is_dir { return Ok(0); }
        let (disk, off) = self.data_start(e)?;
        let raw = RawReader { vols: &self.vols, disk, off, remaining: e.csize };
        let pw = password.map(|p| p.as_bytes());
        // Fast path: whole-buffer libdeflate for plain small deflate entries.
        if e.enc == EncKind::None && e.method == M_DEFLATE && e.csize <= (32 << 20) && e.size <= (crate::sys::default_memory_budget() / crate::sys::worker_count(0) as u64 / 2).min(64 << 20) {
            let mut cbuf = vec![0u8; e.csize as usize];
            self.vols.read_exact(disk, off, &mut cbuf)?;
            let mut obuf = vec![0u8; e.size as usize];
            let mut d = libdeflater::Decompressor::new();
            let n = d.deflate_decompress(&cbuf, &mut obuf).map_err(|er| Error::new(ErrorKind::BrokenArchive, format!("{}: inflate failed ({er:?})", e.name)))?;
            if n as u64 != e.size { return Err(Error::new(ErrorKind::BrokenArchive, format!("{}: size mismatch", e.name))); }
            if crc32fast::hash(&obuf) != e.crc { return Err(Error::new(ErrorKind::CrcMismatch, e.name.clone())); }
            out.write_all(&obuf)?;
            return Ok(n as u64);
        }
        match e.enc {
            EncKind::None => self.decode_stream(e, raw, out, true),
            EncKind::ZipCrypto => {
                let pw = pw.ok_or_else(|| Error::new(ErrorKind::PasswordRequired, e.name.clone()))?;
                let mut raw = raw;
                let mut hdr = [0u8; 12];
                raw.read_exact(&mut hdr)?;
                let mut z = ZipCrypto::new(pw);
                z.decrypt(&mut hdr);
                let check = if e.flags & FLAG_DESCRIPTOR != 0 { (e.dos_time >> 8) as u8 } else { (e.crc >> 24) as u8 };
                let alt = (e.crc >> 24) as u8;
                if hdr[11] != check && hdr[11] != alt { return Err(Error::new(ErrorKind::WrongPassword, e.name.clone())); }
                let dec = ZipCryptoReader { inner: raw, z };
                let mut e2 = e.clone();
                e2.csize = e.csize.saturating_sub(12);
                self.decode_stream(&e2, dec, out, true)
            }
            EncKind::Aes(strength) => {
                let pw = pw.ok_or_else(|| Error::new(ErrorKind::PasswordRequired, e.name.clone()))?;
                let mut raw = raw;
                let sl = crypto::aes_salt_len(strength);
                if e.csize < (sl + 2 + 10) as u64 { return Err(Error::new(ErrorKind::BrokenArchive, "AES entry too short")); }
                let mut sv = vec![0u8; sl + 2];
                raw.read_exact(&mut sv)?;
                let ctx = crypto::check_aes_password(pw, strength, &sv[..sl], [sv[sl], sv[sl + 1]])?;
                let body = e.csize - (sl + 2 + 10) as u64;
                let mut rd = AesReader { inner: raw, ctx: Some(ctx), remaining: body };
                let mut e2 = e.clone();
                e2.csize = body;
                let n = self.decode_stream(&e2, &mut rd, out, !e.aes_ae2)?;
                // drain and authenticate
                let mut sink = [0u8; 4096];
                while rd.remaining > 0 { if rd.read(&mut sink)? == 0 { break; } }
                let mut mac = [0u8; 10];
                rd.inner.read_exact(&mut mac)?;
                if rd.ctx.take().unwrap().finish() != mac { return Err(Error::new(ErrorKind::WrongPassword, format!("{}: authentication failed (wrong password or damaged)", e.name))); }
                Ok(n)
            }
        }
    }

    fn decode_stream<R: Read>(&self, e: &ZipEntry, src: R, out: &mut dyn io::Write, check_crc: bool) -> Result<u64> {
        // Byte-oriented codecs (notably PPMd) must not issue a pread per byte.
        // Buffer the bounded, decrypted payload; AES still drains/authenticates
        // its own bounded ciphertext reader afterwards.
        let mut limited = io::BufReader::with_capacity(256 << 10, src.take(e.csize));
        let mut dec: Box<dyn Read + '_> = match e.method {
            M_PPMD => {
                // 2-byte params: order-1 (bits 0-3), memory MB-1 (bits 4-11), restore method (bits 12-15)
                let mut h = [0u8; 2];
                limited.read_exact(&mut h)?;
                let w = u16::from_le_bytes(h);
                let (order, mem, rm) = (((w & 0xF) + 1) as u32, ((((w >> 4) & 0xFF) as u32) + 1) << 20, (w >> 12) as u32);
                if mem as u64 > crate::sys::default_memory_budget() { return Err(Error::new(ErrorKind::DictionaryTooLarge, format!("{}: PPMd model needs {} MB", e.name, mem >> 20))); }
                Box::new(ppmd_rust::Ppmd8Decoder::new(limited, order, mem, rm.into()).map_err(|er| Error::new(ErrorKind::BrokenArchive, format!("{}: {er:?}", e.name)))?)
            }
            M_DEFLATE64 => Box::new(deflate64::Deflate64Decoder::new(limited)),
            M_LZMA => {
                // ZIP LZMA: 2-byte version, 2-byte props size, props, raw stream -> rebuild a .lzma header
                let mut h = [0u8; 4];
                limited.read_exact(&mut h)?;
                let psz = rd16(&h, 2) as usize;
                if psz != 5 { return Err(Error::new(ErrorKind::BrokenArchive, format!("{}: bad LZMA properties", e.name))); }
                let mut props = [0u8; 5];
                limited.read_exact(&mut props)?;
                let dict = u32::from_le_bytes([props[1], props[2], props[3], props[4]]) as u64;
                if dict > crate::sys::default_memory_budget() { return Err(Error::new(ErrorKind::DictionaryTooLarge, format!("{}: LZMA dictionary {} MB exceeds the memory budget", e.name, dict >> 20))); }
                let mut head = props.to_vec();
                head.extend_from_slice(&(if e.flags & 2 != 0 { u64::MAX } else { e.size }).to_le_bytes());
                let s = xz2::stream::Stream::new_lzma_decoder(crate::sys::default_memory_budget())
                    .map_err(|er| Error::new(ErrorKind::Io, er.to_string()))?;
                Box::new(xz2::read::XzDecoder::new_stream(io::Cursor::new(head).chain(limited), s))
            }
            M_STORE => Box::new(limited),
            M_DEFLATE => Box::new(flate2::read::DeflateDecoder::new(limited)),
            M_BZIP2 => Box::new(bzip2::read::BzDecoder::new(limited)),
            M_ZSTD => Box::new(zstd::stream::read::Decoder::new(limited)?),
            m => return Err(Error::new(ErrorKind::UnsupportedMethod, format!("{}: compression method {m}", e.name))),
        };
        let mut h = crc32fast::Hasher::new();
        let mut buf = vec![0u8; 256 << 10];
        let mut total = 0u64;
        loop {
            let n = dec.read(&mut buf).map_err(|er| Error::new(ErrorKind::BrokenArchive, format!("{}: {er}", e.name)))?;
            if n == 0 { break; }
            if (n as u64) > e.size.saturating_sub(total) {
                return Err(Error::new(ErrorKind::BrokenArchive, format!("{}: expanded beyond declared size", e.name)));
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            total += n as u64;
        }
        if total != e.size { return Err(Error::new(ErrorKind::BrokenArchive, format!("{}: size mismatch", e.name))); }
        if check_crc && h.finalize() != e.crc { return Err(Error::new(ErrorKind::CrcMismatch, e.name.clone())); }
        Ok(total)
    }
}

struct RawReader<'a> { vols: &'a Vols, disk: u32, off: u64, remaining: u64 }
impl Read for RawReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = (buf.len() as u64).min(self.remaining) as usize;
        if n == 0 { return Ok(0); }
        let r = self.vols.read_at(self.disk, self.off, &mut buf[..n])?;
        if r == 0 { return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "archive truncated")); }
        let (d, o) = self.vols.advance(self.disk, self.off, r as u64);
        self.disk = d; self.off = o; self.remaining -= r as u64;
        Ok(r)
    }
}
struct ZipCryptoReader<R> { inner: R, z: ZipCrypto }
impl<R: Read> Read for ZipCryptoReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> { let n = self.inner.read(buf)?; self.z.decrypt(&mut buf[..n]); Ok(n) }
}
struct AesReader<R> { inner: R, ctx: Option<AesCtx>, remaining: u64 }
impl<R: Read> Read for AesReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = (buf.len() as u64).min(self.remaining) as usize;
        if n == 0 { return Ok(0); }
        let r = self.inner.read(&mut buf[..n])?;
        if let Some(c) = self.ctx.as_mut() { c.decrypt(&mut buf[..r]); }
        self.remaining -= r as u64;
        Ok(r)
    }
}

/// Fast password rejection data (see [`ZipArchive::verifier`]).
#[derive(Clone, Debug)]
pub enum PasswordVerifier {
    ZipCrypto { header: [u8; 12], check: u8, alt: u8, entry: usize },
    Aes { strength: u8, salt: Vec<u8>, verifier: [u8; 2], entry: usize },
}
impl PasswordVerifier {
    pub fn entry(&self) -> usize { match self { PasswordVerifier::ZipCrypto { entry, .. } | PasswordVerifier::Aes { entry, .. } => *entry } }
    /// True if `pw` passes the cheap check (may be a false positive: 1/256 ZipCrypto, 1/65536 AES).
    pub fn check(&self, pw: &[u8]) -> bool {
        match self {
            PasswordVerifier::ZipCrypto { header, check, alt, .. } => {
                let mut h = *header;
                ZipCrypto::new(pw).decrypt(&mut h);
                h[11] == *check || h[11] == *alt
            }
            PasswordVerifier::Aes { strength, salt, verifier, .. } => crypto::check_aes_password(pw, *strength, salt, *verifier).is_ok(),
        }
    }
}
