//! TAR family: create (parallel read-ahead + parallel codecs) and extract/list.
use super::codecs::{self, Codec};
use super::common::{apply_meta, plan_targets, resolve_conflict};
use crate::cancel::{CancelToken, Progress};
use crate::enumerate::{EntryDesc, EntryKind};
use crate::error::{Error, ErrorKind, Result};
use crate::pathsafe;
use crate::zip::extract::{ConflictChoice, ExtractOptions, ExtractReport};
use crate::zip::Level;
use rayon::prelude::*;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct TarOptions {
    pub codec: Codec,
    pub level: Level,
    pub threads: usize,
    pub store_symlinks: bool,
    pub store_hardlinks: bool,
    pub memory_budget: u64,
}
impl Default for TarOptions {
    fn default() -> Self { TarOptions { codec: Codec::None, level: Level::Normal, threads: 0, store_symlinks: true, store_hardlinks: true, memory_budget: crate::sys::default_memory_budget() } }
}

const SMALL: u64 = 8 << 20;

pub fn create_tar(out: &Path, entries: &[EntryDesc], o: &TarOptions, cancel: &CancelToken, progress: &Progress) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    progress.set_totals(entries.len() as u64, entries.iter().map(|e| e.size).sum());
    progress.set_phase("compressing");
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let part = tempfile::Builder::new().prefix(".osz-create-").tempfile_in(parent)?.into_temp_path();
    let result = (|| -> Result<u64> {
        let f = BufWriter::with_capacity(1 << 20, File::create(&part)?);
        let comp = codecs::compressor(o.codec, o.level, o.threads, f)?;
        let mut b = tar::Builder::new(comp);
        b.mode(tar::HeaderMode::Complete);
        let pool = crate::sys::build_pool(o.threads);
        let mut inodes: std::collections::HashMap<(u64, u64), String> = Default::default();
        let batch_cap = (o.memory_budget / 8).clamp(8 << 20, 128 << 20);
        let mut i = 0;
        while i < entries.len() {
            cancel.check()?;
            // gather a batch of small regular files to prefetch in parallel
            let mut j = i;
            let mut bytes = 0u64;
            while j < entries.len() && bytes < batch_cap && (entries[j].kind != EntryKind::File || entries[j].size < SMALL) {
                bytes += entries[j].size; j += 1;
            }
            if j == i { j = i + 1; }
            let batch = &entries[i..j];
            let data: Vec<Option<Vec<u8>>> = pool.install(|| batch.par_iter().map(|d| {
                if d.kind == EntryKind::File && d.size < SMALL { std::fs::read(&d.abs).ok() } else { None }
            }).collect());
            for (d, buf) in batch.iter().zip(data) {
                cancel.check()?;
                progress.set_current(&d.name);
                let mut h = tar::Header::new_gnu();
                h.set_mtime(crate::zip::format::unix_secs(d.mtime).max(0) as u64);
                h.set_uid(0); h.set_gid(0);
                let name = d.name.trim_end_matches('/');
                match d.kind {
                    EntryKind::Dir => {
                        h.set_entry_type(tar::EntryType::Directory); h.set_mode(if d.mode & 0o7777 == 0 { 0o755 } else { d.mode & 0o7777 }); h.set_size(0);
                        b.append_data(&mut h, format!("{name}/"), std::io::empty())?;
                    }
                    EntryKind::Symlink => {
                        h.set_entry_type(tar::EntryType::Symlink); h.set_mode(0o777); h.set_size(0);
                        b.append_link(&mut h, name, d.link_target.clone().unwrap_or_default())?;
                    }
                    EntryKind::File => {
                        h.set_entry_type(tar::EntryType::Regular);
                        h.set_mode(if d.mode & 0o7777 == 0 { 0o644 } else { d.mode & 0o7777 });
                        if o.store_hardlinks {
                            if let Ok(m) = std::fs::metadata(&d.abs) {
                                if m.nlink() > 1 {
                                    if let Some(first) = inodes.get(&(m.dev(), m.ino())) {
                                        h.set_entry_type(tar::EntryType::Link); h.set_size(0);
                                        b.append_link(&mut h, name, first)?;
                                        progress.file_done();
                                        continue;
                                    }
                                    inodes.insert((m.dev(), m.ino()), name.to_string());
                                }
                            }
                        }
                        match buf {
                            Some(v) => { h.set_size(v.len() as u64); progress.add_read(v.len() as u64); b.append_data(&mut h, name, v.as_slice())?; }
                            None => {
                                let size = std::fs::metadata(&d.abs)?.len();
                                h.set_size(size);
                                let r = CancelRead { r: File::open(&d.abs)?, c: cancel, p: progress };
                                b.append_data(&mut h, name, r)?;
                            }
                        }
                    }
                }
                progress.file_done();
            }
            i = j;
        }
        let comp = b.into_inner()?;
        comp.finish()?;
        let f = File::open(&part)?;
        f.sync_all()?;
        let len = f.metadata()?.len();
        std::fs::rename(&part, out)?;
        Ok(len)
    })();
    if result.is_err() { let _ = std::fs::remove_file(&part); }
    result
}

struct CancelRead<'a, R: Read> { r: R, c: &'a CancelToken, p: &'a Progress }
impl<R: Read> Read for CancelRead<'_, R> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if self.c.is_cancelled() { return Err(crate::cancel::io_cancelled()); }
        let n = self.r.read(b)?; self.p.add_read(n as u64); Ok(n)
    }
}

#[derive(Debug, Clone)]
pub struct TarInfo { pub name: String, pub size: u64, pub mtime: i64, pub mode: u32, pub is_dir: bool, pub is_symlink: bool, pub link: Option<String> }

/// Detect the compression codec of a path (content first, extension as hint).
pub fn sniff_codec(path: &Path) -> Result<Codec> {
    let mut head = [0u8; 16];
    let n = File::open(path)?.read(&mut head)?;
    if let Some(c) = Codec::from_magic(&head[..n]) { return Ok(c); }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("lzma") || ext.eq_ignore_ascii_case("tlz") { return Ok(Codec::Lzma); }
    if ext.eq_ignore_ascii_case("br") { return Ok(Codec::Brotli); }
    Ok(Codec::None)
}

fn open_tar(path: &Path, mem: u64) -> Result<tar::Archive<Box<dyn Read + Send>>> {
    let codec = sniff_codec(path)?;
    let r = codecs::decompressor(codec, BufReader::with_capacity(1 << 20, File::open(path)?), mem)?;
    Ok(tar::Archive::new(r))
}

/// True when `path` (after decompression) is a tar stream.
pub fn is_tar(path: &Path) -> bool {
    let Ok(codec) = sniff_codec(path) else { return false };
    let Ok(f) = File::open(path) else { return false };
    let Ok(mut r) = codecs::decompressor(codec, f, 256 << 20) else { return false };
    let mut b = [0u8; 512];
    let mut n = 0;
    while n < 512 { match r.read(&mut b[n..]) { Ok(0) | Err(_) => break, Ok(k) => n += k } }
    n == 512 && &b[257..262] == b"ustar" || (n == 512 && header_checksum_ok(&b))
}
fn header_checksum_ok(b: &[u8; 512]) -> bool {
    let s = std::str::from_utf8(&b[148..156]).unwrap_or("").trim_matches(|c: char| c == '\0' || c == ' ');
    let Ok(want) = u32::from_str_radix(s, 8) else { return false };
    let sum: u32 = b.iter().enumerate().map(|(i, &x)| if (148..156).contains(&i) { 32 } else { x as u32 }).sum();
    sum == want && b[0] != 0
}

pub fn list_tar(path: &Path, mem: u64, cancel: &CancelToken) -> Result<Vec<TarInfo>> {
    let mut a = open_tar(path, mem)?;
    let mut v = Vec::new();
    for e in a.entries().map_err(codecs::map_decode_err)? {
        cancel.check()?;
        let e = e.map_err(codecs::map_decode_err)?;
        let h = e.header();
        let t = h.entry_type();
        v.push(TarInfo {
            name: e.path().map_err(codecs::map_decode_err)?.to_string_lossy().into_owned(), size: h.size().unwrap_or(0), mtime: h.mtime().unwrap_or(0) as i64,
            mode: h.mode().unwrap_or(0o644), is_dir: t.is_dir(), is_symlink: t.is_symlink() || t.is_hard_link(),
            link: e.link_name().ok().flatten().map(|l| l.to_string_lossy().into_owned()),
        });
    }
    Ok(v)
}

pub fn extract_tar(path: &Path, o: &ExtractOptions, cancel: &CancelToken, progress: &Progress) -> Result<ExtractReport> {
    let mem = crate::sys::default_memory_budget();
    let mut report = ExtractReport::default();
    // pass 1: names (needed for smart extract / filters)
    let infos = list_tar(path, mem, cancel)?;
    let names: Vec<(String, bool)> = infos.iter().map(|i| (i.name.clone(), i.is_dir)).collect();
    let (dest, targets) = plan_targets(&names, path, o, &mut report)?;
    progress.set_totals(infos.len() as u64, infos.iter().map(|i| i.size).sum());
    progress.set_phase("extracting");
    let mut a = open_tar(path, mem)?;
    let mut created: Vec<std::path::PathBuf> = Vec::new();
    let mut dirs: Vec<(std::path::PathBuf, u32, i64)> = Vec::new();
    let mut hardlinks: Vec<(std::path::PathBuf, String)> = Vec::new();
    let ask = |p: &Path| o.on_conflict.as_ref().map(|f| f(p, None)).unwrap_or(ConflictChoice::Skip);
    let res: Result<()> = (|| {
        for (idx, e) in a.entries().map_err(codecs::map_decode_err)?.enumerate() {
            cancel.check()?;
            let mut e = e.map_err(codecs::map_decode_err)?;
            let Some(t) = targets.get(idx).and_then(|t| t.clone()) else { continue };
            let info = &infos[idx];
            if pathsafe::ensure_no_symlink(&dest, &t.path).is_err() { report.unsafe_refused.push(info.name.clone()); continue; }
            progress.set_current(&t.rel);
            let ty = e.header().entry_type();
            if ty.is_dir() { std::fs::create_dir_all(&t.path)?; dirs.push((t.path.clone(), info.mode, info.mtime)); continue; }
            if let Some(p) = t.path.parent() { std::fs::create_dir_all(p)?; }
            if ty.is_symlink() {
                let target = info.link.clone().unwrap_or_default();
                let parent_rel = Path::new(&t.rel).parent().unwrap_or(Path::new("")).to_path_buf();
                if !o.allow_external_symlinks && !pathsafe::symlink_target_safe(&dest, &parent_rel, &target) { report.unsafe_refused.push(format!("{} -> {}", info.name, target)); continue; }
                if let Some(p) = resolve_conflict(&t.path, o, &ask) { let _ = std::fs::remove_file(&p); std::os::unix::fs::symlink(&target, &p)?; report.extracted += 1; }
                continue;
            }
            if ty.is_hard_link() { hardlinks.push((t.path.clone(), info.link.clone().unwrap_or_default())); continue; }
            if !ty.is_file() { continue; }
            let Some(p) = resolve_conflict(&t.path, o, &ask) else { report.skipped += 1; progress.file_done(); continue };
            let existed = p.exists();
            let temporary = tempfile::NamedTempFile::new_in(p.parent().unwrap_or(&dest))?;
            let f = temporary.as_file();
            if info.size > (1 << 20) { let _ = f.set_len(info.size); }
            let mut w = BufWriter::with_capacity(1 << 20, f);
            let mut buf = vec![0u8; 256 << 10];
            let mut n = 0u64;
            loop {
                cancel.check()?;
                let k = e.read(&mut buf).map_err(codecs::map_decode_err)?;
                if k == 0 { break; }
                w.write_all(&buf[..k])?; n += k as u64;
            }
            w.flush()?; drop(w);
            temporary.persist(&p).map_err(|e| Error::from(e.error))?;
            if !existed { created.push(p.clone()); }
            apply_meta(&p, info.mode, info.mtime, false);
            progress.add_written(n); progress.file_done();
            report.extracted += 1; report.bytes += n;
        }
        Ok(())
    })();
    if let Err(e) = res {
        if e.kind == ErrorKind::Cancelled && o.delete_partial_on_cancel { for p in &created { let _ = std::fs::remove_file(p); } }
        return Err(e);
    }
    // hard links last (targets exist now)
    for (p, link) in hardlinks {
        let normalized = pathsafe::normalize_read_name(&link);
        let src_rel = normalized.as_str();
        let strip_ok = pathsafe::safe_join(&dest, src_rel).or_else(|_| Err(()));
        let Ok(src) = strip_ok else { report.unsafe_refused.push(format!("hardlink {link}")); continue };
        let src = if src.exists() { src } else {
            // smart-stripped archives: try again relative to the root-stripped layout
            match link.split_once('/') { Some((_, rest)) => pathsafe::safe_join(&dest, rest)?, None => src }
        };
        if pathsafe::ensure_no_symlink(&dest, &src).is_err() || pathsafe::ensure_no_symlink(&dest, &p).is_err() {
            report.unsafe_refused.push(format!("hardlink {link}")); continue;
        }
        let Some(p) = resolve_conflict(&p, o, &ask) else { report.skipped += 1; continue; };
        let _ = std::fs::remove_file(&p);
        if std::fs::hard_link(&src, &p).is_ok() { report.extracted += 1; }
    }
    dirs.sort_by_key(|d| std::cmp::Reverse(d.0.components().count()));
    for (p, m, t) in dirs { apply_meta(&p, m, t, true); }
    Ok(report)
}

/// Decompress a single-stream file (.gz/.xz/.zst/...) to `out_dir/<stem>`.
pub fn extract_single_stream(path: &Path, out_dir: &Path, codec: Codec, cancel: &CancelToken, progress: &Progress) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let target = pathsafe::unique_name(&out_dir.join(stem));
    let mut r = codecs::decompressor(codec, BufReader::with_capacity(1 << 20, File::open(path)?), crate::sys::default_memory_budget())?;
    let mut w = BufWriter::with_capacity(1 << 20, File::create(&target)?);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.is_cancelled() { drop(w); let _ = std::fs::remove_file(&target); return Err(Error::new(ErrorKind::Cancelled, "cancelled")); }
        let n = r.read(&mut buf).map_err(codecs::map_decode_err)?;
        if n == 0 { break; }
        w.write_all(&buf[..n])?; progress.add_written(n as u64);
    }
    w.flush()?;
    Ok(target)
}

/// Compress one file as a single stream (`name.ext`).
pub fn create_single_stream(out: &Path, input: &Path, codec: Codec, level: Level, threads: usize, cancel: &CancelToken, progress: &Progress) -> Result<u64> {
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let part = tempfile::Builder::new().prefix(".osz-create-").tempfile_in(parent)?.into_temp_path();
    let r = (|| -> Result<u64> {
        let mut c = codecs::compressor(codec, level, threads, BufWriter::with_capacity(1 << 20, File::create(&part)?))?;
        let mut f = File::open(input)?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            cancel.check()?;
            let n = f.read(&mut buf)?;
            if n == 0 { break; }
            c.write_all(&buf[..n])?; progress.add_read(n as u64);
        }
        c.finish()?;
        let l = std::fs::metadata(&part)?.len();
        std::fs::rename(&part, out)?;
        Ok(l)
    })();
    if r.is_err() { let _ = std::fs::remove_file(&part); }
    r
}
