//! 7z create/list/extract on top of sevenz-rust2, parallel at block level.
//! Solid blocks are compressed concurrently (`prepare_block`) and appended in order; a single
//! large block uses multithreaded LZMA2 chunks instead.
use super::common::{apply_meta, plan_targets, resolve_conflict};
use crate::cancel::{CancelToken, Progress};
use crate::enumerate::{EntryDesc, EntryKind};
use crate::error::{Error, ErrorKind, Result};
use crate::skiplist::SkipList;
use crate::zip::extract::{ConflictChoice, ExtractOptions, ExtractReport};
use crate::zip::Level;
use rayon::prelude::*;
use sevenz_rust2 as sz;
use sevenz_rust2::encoder_options as eo;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SzMethod { Lzma2, Lzma, Ppmd, Bzip2, Zstd, Deflate, Copy }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Solid { Auto, None, Single, Bytes(u64) }

#[derive(Clone, Debug)]
pub struct SevenOptions {
    pub method: SzMethod,
    pub level: Level,
    pub solid: Solid,
    pub password: Option<String>,
    pub encrypt_names: bool,
    pub threads: usize,
    pub dict_size: Option<u32>,
    pub high_speed: bool,
    pub skip: SkipList,
    pub split_size: Option<u64>,
    pub memory_budget: u64,
}
impl Default for SevenOptions {
    fn default() -> Self {
        SevenOptions { method: SzMethod::Lzma2, level: Level::Normal, solid: Solid::Auto, password: None, encrypt_names: false, threads: 0,
            dict_size: None, high_speed: true, skip: SkipList::default(), split_size: None, memory_budget: crate::sys::default_memory_budget() }
    }
}

fn lvl(l: Level) -> u32 { match l { Level::Store => 0, Level::Fast => 3, Level::Normal => 5, Level::Maximum => 9 } }
fn dict_for(l: Level) -> u32 { match l { Level::Store => 1 << 16, Level::Fast => 1 << 20, Level::Normal => 16 << 20, Level::Maximum => 64 << 20 } }

pub fn map_err(e: sz::Error) -> Error { sz_err(e) }

fn sz_err(e: sz::Error) -> Error {
    use sz::Error as E;
    let s = e.to_string();
    match &e {
        E::PasswordRequired => Error::new(ErrorKind::PasswordRequired, s),
        E::MaybeBadPassword(_) => Error::new(ErrorKind::WrongPassword, s),
        E::MaxMemLimited { .. } => Error::new(ErrorKind::DictionaryTooLarge, s),
        E::UnsupportedCompressionMethod(_) | E::Unsupported(_) | E::ExternalUnsupported => Error::new(ErrorKind::UnsupportedMethod, s),
        E::ChecksumVerificationFailed => Error::new(ErrorKind::CrcMismatch, s),
        E::BadSignature(_) | E::UnsupportedVersion { .. } => Error::new(ErrorKind::UnsupportedFormat, s),
        E::Io(io, _) | E::FileOpen(io, _) => Error::new(Error::from(std::io::Error::new(io.kind(), io.to_string())).kind, s),
        _ => Error::new(ErrorKind::BrokenArchive, s),
    }
}

fn methods(o: &SevenOptions, threads: u32, chunk: u64) -> Vec<sz::EncoderConfiguration> {
    let l = lvl(o.level);
    let main: sz::EncoderConfiguration = if o.level == Level::Store || o.method == SzMethod::Copy { sz::EncoderMethod::COPY.into() } else {
        match o.method {
            SzMethod::Lzma2 => {
                let mut x = if threads > 1 { eo::Lzma2Options::from_level_mt(l, threads, chunk) } else { eo::Lzma2Options::from_level(l) };
                x.set_dictionary_size(o.dict_size.unwrap_or(dict_for(o.level)));
                x.into()
            }
            SzMethod::Lzma => { let mut x = eo::LzmaOptions::from_level(l); x.set_dictionary_size(o.dict_size.unwrap_or(dict_for(o.level))); x.into() }
            SzMethod::Ppmd => eo::PpmdOptions::from_level(match o.level { Level::Fast => 3, Level::Maximum => 8, _ => 6 }).into(),
            SzMethod::Bzip2 => eo::Bzip2Options::from_level(l.max(1)).into(),
            SzMethod::Zstd => eo::ZstandardOptions::from_level(match o.level { Level::Fast => 3, Level::Maximum => 19, _ => 9 }).into(),
            SzMethod::Deflate => eo::DeflateOptions::from_level(l.max(1)).into(),
            SzMethod::Copy => sz::EncoderMethod::COPY.into(),
        }
    };
    let mut v = Vec::new();
    if let Some(p) = &o.password { v.push(eo::AesEncoderOptions::new(sz::Password::from(p.as_str())).into()); }
    v.push(main);
    v
}

struct LazyCancelRead { path: PathBuf, f: Option<File>, c: CancelToken, p: Progress }
impl Read for LazyCancelRead {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if self.c.is_cancelled() { return Err(crate::cancel::io_cancelled()); }
        if self.f.is_none() { self.f = Some(File::open(&self.path)?); }
        let n = self.f.as_mut().unwrap().read(b)?;
        self.p.add_read(n as u64);
        Ok(n)
    }
}

fn entry_for(d: &EntryDesc) -> sz::ArchiveEntry {
    let name = d.name.trim_end_matches('/').to_string();
    let mut e = sz::ArchiveEntry::from_path(&d.abs, name);
    if d.kind == EntryKind::Dir { e.is_directory = true; e.has_stream = false; }
    // DOS attribute bits for Windows + the p7zip/7-Zip unix extension (0x8000 flag, mode in the high 16 bits)
    let dos: u32 = if d.kind == EntryKind::Dir { 0x10 } else { 0x20 };
    let type_bits: u32 = match d.kind { EntryKind::Dir => 0o040000, _ => 0o100000 };
    e.has_windows_attributes = true;
    e.windows_attributes = dos | 0x8000 | (((d.mode & 0o7777) | type_bits) << 16);
    e
}

pub fn create_7z(out: &Path, entries: &[EntryDesc], o: &SevenOptions, cancel: &CancelToken, progress: &Progress) -> Result<u64> {
    cancel.check()?;
    if entries.iter().any(|e| e.kind == EntryKind::Symlink) {
        return Err(Error::new(ErrorKind::UnsupportedFormat, "7z symlink storage is unsupported; use ZIP/TAR or explicitly dereference links"));
    }
    if let Some(s) = o.split_size { if s < crate::zip::writer::MIN_SPLIT { return Err(Error::new(ErrorKind::InvalidSplitSize, "minimum volume size is 100 KB")); } }
    progress.set_totals(entries.len() as u64, entries.iter().map(|e| e.size).sum());
    progress.set_phase("compressing");
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let part = tempfile::Builder::new().prefix(".osz-create-").tempfile_in(parent)?.into_temp_path();
    let r = (|| -> Result<u64> {
        // liblzma encodes each solid block on one worker. Size automatic blocks to
        // expose useful P-core parallelism; cap workers using estimated codec workspace.
        let requested = crate::sys::worker_count(o.threads);
        let source_bytes = entries.iter().map(|e| e.size).sum::<u64>();
        let block_limit = match o.solid {
            Solid::Auto => source_bytes.div_ceil(requested as u64).clamp(16 << 20, 64 << 20),
            Solid::None => 0, Solid::Single => u64::MAX, Solid::Bytes(b) => b,
        };
        let dictionary = o.dict_size.unwrap_or(dict_for(o.level)) as u64;
        let workspace = dictionary.saturating_mul(12).saturating_add(32 << 20)
            .saturating_add(block_limit.min(source_bytes).saturating_mul(2));
        let memory_workers = (o.memory_budget / workspace.max(1)).max(1) as usize;
        let threads = requested.min(memory_workers);
        let pool = crate::sys::build_pool(threads);
        let mut w = sz::ArchiveWriter::new(File::create(&part)?).map_err(sz_err)?;
        w.set_encrypt_header(o.encrypt_names && o.password.is_some());

        // partition: dirs/empty files, stored (skip-list), compressible
        let mut empties = Vec::new();
        let mut stored: Vec<&EntryDesc> = Vec::new();
        let mut comp: Vec<&EntryDesc> = Vec::new();
        for d in entries {
            match d.kind {
                EntryKind::Dir => empties.push(d),
                _ if d.size == 0 && d.kind == EntryKind::File => empties.push(d),
                _ if o.level == Level::Store || o.method == SzMethod::Copy || (o.high_speed && o.skip.should_store(&d.name)) => stored.push(d),
                _ => comp.push(d),
            }
        }
        for d in empties {
            let mut e = entry_for(d);
            if d.kind == EntryKind::File { e.has_stream = false; e.is_directory = false; }
            w.push_archive_entry::<&[u8]>(e, None).map_err(sz_err)?;
        }
        let group = |list: &[&EntryDesc]| -> Vec<Vec<usize>> {
            let mut g: Vec<Vec<usize>> = Vec::new();
            let mut cur: Vec<usize> = Vec::new();
            let mut sum = 0u64;
            for (i, d) in list.iter().enumerate() {
                if !cur.is_empty() && (block_limit == 0 || sum + d.size > block_limit) { g.push(std::mem::take(&mut cur)); sum = 0; }
                cur.push(i); sum += d.size;
            }
            if !cur.is_empty() { g.push(cur); }
            g
        };
        let mk_reader = |d: &EntryDesc| sz::SourceReader::from(LazyCancelRead { path: d.abs.clone(), f: None, c: cancel.clone(), p: progress.clone() });
        let wrap_err = |e: sz::Error| if cancel.is_cancelled() { Error::new(ErrorKind::Cancelled, "cancelled") } else { sz_err(e) };

        for (list, is_store) in [(&stored, true), (&comp, false)] {
            let groups = group(list);
            if groups.is_empty() { continue; }
            // parallel across blocks when there are enough; otherwise one MT-LZMA2 stream per block
            let across = groups.len() >= 2 && threads > 1;
            let m = if is_store {
                Arc::new(methods(&SevenOptions { level: Level::Store, ..o.clone() }, 1, 0))
            } else if across {
                Arc::new(methods(o, 1, 0))
            } else {
                Arc::new(methods(o, threads as u32, (dict_for(o.level) as u64).max(4 << 20)))
            };
            if across || is_store {
                let wave = threads.max(1);
                for chunk in groups.chunks(wave) {
                    cancel.check()?;
                    let prepared: Vec<std::result::Result<sz::PreparedBlock, sz::Error>> = pool.install(|| chunk.par_iter().map(|g| {
                        let es: Vec<_> = g.iter().map(|&i| entry_for(list[i])).collect();
                        let rs: Vec<_> = g.iter().map(|&i| mk_reader(list[i])).collect();
                        sz::prepare_block(m.clone(), es, rs)
                    }).collect());
                    for p in prepared {
                        w.push_prepared_block(p.map_err(wrap_err)?).map_err(sz_err)?;
                    }
                    for g in chunk { for _ in g { progress.file_done(); } }
                }
            } else {
                w.set_content_methods((*m).clone());
                for g in &groups {
                    cancel.check()?;
                    let es: Vec<_> = g.iter().map(|&i| entry_for(list[i])).collect();
                    let rs: Vec<_> = g.iter().map(|&i| mk_reader(list[i])).collect();
                    w.push_archive_entries(es, rs).map_err(wrap_err)?;
                    for _ in g { progress.file_done(); }
                }
            }
        }
        let f = w.finish()?;
        f.sync_all()?;
        let len = f.metadata()?.len();
        drop(f);
        match o.split_size {
            None => { std::fs::rename(&part, out)?; Ok(len) }
            Some(sp) => { split_file(&part, out, sp)?; let _ = std::fs::remove_file(&part); Ok(len) }
        }
    })();
    if r.is_err() { let _ = std::fs::remove_file(&part); }
    r
}

/// Byte-split `src` into `out.001`, `out.002`, ... (the 7-Zip volume layout).
pub fn split_file(src: &Path, out: &Path, size: u64) -> Result<Vec<PathBuf>> {
    let mut f = File::open(src)?;
    let total = f.metadata()?.len();
    let mut names = Vec::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut idx = 1;
    let mut left = total;
    loop {
        let p = PathBuf::from(format!("{}.{:03}", out.display(), idx));
        let mut w = std::io::BufWriter::new(File::create(&p)?);
        let mut vol = size.min(left);
        left -= vol;
        while vol > 0 {
            let want = (vol as usize).min(buf.len());
            let n = f.read(&mut buf[..want])?;
            if n == 0 { break; }
            w.write_all(&buf[..n])?; vol -= n as u64;
        }
        w.flush()?;
        names.push(p);
        idx += 1;
        if left == 0 { break; }
    }
    Ok(names)
}

/// Join `.001/.002/...` volumes into one readable file in `tmp`. Returns the joined path.
pub fn join_volumes(first: &Path, tmp: &Path) -> Result<PathBuf> {
    let s = first.to_string_lossy();
    let base = s.rsplit_once('.').map(|x| x.0.to_string()).ok_or_else(|| Error::new(ErrorKind::InvalidInput, "not a volume"))?;
    let out = tmp.join(Path::new(&base).file_name().unwrap());
    let mut w = std::io::BufWriter::new(File::create(&out)?);
    for i in 1..100000 {
        let p = PathBuf::from(format!("{base}.{i:03}"));
        if !p.exists() {
            if i == 1 { return Err(Error::new(ErrorKind::MissingVolume, format!("missing {}", p.display()))); }
            break;
        }
        std::io::copy(&mut File::open(&p)?, &mut w)?;
    }
    w.flush()?;
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct SzInfo { pub name: String, pub size: u64, pub csize: u64, pub mtime: i64, pub is_dir: bool, pub crc: u32, pub attrs: u32 }

/// Unix mode from 7z attributes (0 => let apply_meta pick the default).
fn unix_mode(attr: u32, _dir: bool) -> u32 { if attr & 0x8000 != 0 { (attr >> 16) & 0o7777 } else { 0 } }

fn nt_to_unix(t: sz::NtTime) -> i64 { (u64::from(t) / 10_000_000) as i64 - 11_644_473_600 }

pub fn open_archive(path: &Path, password: Option<&str>) -> Result<sz::Archive> {
    let pw = password.map(sz::Password::from).unwrap_or_else(sz::Password::empty);
    sz::Archive::open_with_password(path, &pw).map_err(|e| {
        let k = sz_err(e);
        if k.kind == ErrorKind::WrongPassword && password.is_none() { Error::new(ErrorKind::PasswordRequired, k.detail) } else { k }
    })
}

pub fn list_7z(path: &Path, password: Option<&str>) -> Result<Vec<SzInfo>> {
    let a = open_archive(path, password)?;
    Ok(a.files.iter().map(|f| SzInfo { name: f.name().to_string(), size: f.size(), csize: f.compressed_size, mtime: nt_to_unix(f.last_modified_date()),
        is_dir: f.is_directory(), crc: f.crc as u32, attrs: f.windows_attributes() }).collect())
}

pub fn extract_7z(path: &Path, o: &ExtractOptions, cancel: &CancelToken, progress: &Progress) -> Result<ExtractReport> {
    let archive = open_archive(path, o.password.as_deref())?;
    let pw = o.password.as_deref().map(sz::Password::from).unwrap_or_else(sz::Password::empty);
    let mut report = ExtractReport::default();
    let names: Vec<(String, bool)> = archive.files.iter().map(|f| (f.name().to_string(), f.is_directory())).collect();
    let mut unique = std::collections::HashSet::new();
    if names.iter().any(|(name, _)| !unique.insert(name)) {
        return Err(Error::new(ErrorKind::UnsafePath, "duplicate 7z entry names"));
    }
    let infos: Vec<(u32, i64)> = archive.files.iter().map(|f| (f.windows_attributes(), nt_to_unix(f.last_modified_date()))).collect();
    let (_dest, targets) = plan_targets(&names, path, o, &mut report)?;
    progress.set_totals(names.len() as u64, archive.files.iter().map(|f| f.size()).sum());
    progress.set_phase("extracting");
    use std::collections::HashMap;
    let by_name: HashMap<&str, usize> = names.iter().enumerate().map(|(i, (n, _))| (n.as_str(), i)).collect();
    for (i, t) in targets.iter().enumerate() {
        if let (Some(t), true) = (t, names[i].1) { std::fs::create_dir_all(&t.path)?; }
    }
    let ask = |p: &Path| o.on_conflict.as_ref().map(|f| f(p, None)).unwrap_or(ConflictChoice::Skip);
    let stats = std::sync::Mutex::new((0u64, 0u64, 0u64, Vec::<(String, String)>::new(), Vec::<PathBuf>::new()));
    let blocks = archive.blocks.len();
    let pool = crate::sys::build_pool(o.threads);
    let handle_entry = |name: &str, rd: &mut dyn Read| -> std::result::Result<bool, sz::Error> {
        if cancel.is_cancelled() { return Err(sz::Error::Other(std::borrow::Cow::Borrowed("cancelled"))); }
        let Some(&idx) = by_name.get(name) else { return Ok(true) };
        let Some(t) = &targets[idx] else { return Ok(true) };
        if names[idx].1 { return Ok(true); }
        progress.set_current(&t.rel);
        if let Some(p) = t.path.parent() { let _ = std::fs::create_dir_all(p); }
        let Some(p) = resolve_conflict(&t.path, o, &ask) else { stats.lock().unwrap().1 += 1; progress.file_done(); return Ok(true) };
        let existed = p.exists();
        let temporary = tempfile::NamedTempFile::new_in(p.parent().unwrap_or(Path::new(".")))
            .map_err(|e| sz::Error::Io(e, "create".into()))?;
        let f = temporary.as_file();
        let mut w = std::io::BufWriter::with_capacity(1 << 20, f);
        let mut buf = vec![0u8; 256 << 10];
        let mut n = 0u64;
        loop {
            if cancel.is_cancelled() { return Err(sz::Error::Other(std::borrow::Cow::Borrowed("cancelled"))); }
            let k = rd.read(&mut buf).map_err(|e| sz::Error::Io(e, "read".into()))?;
            if k == 0 { break; }
            w.write_all(&buf[..k]).map_err(|e| sz::Error::Io(e, "write".into()))?;
            n += k as u64;
        }
        w.flush().map_err(|e| sz::Error::Io(e, "flush".into()))?;
        drop(w);
        temporary.persist(&p).map_err(|e| sz::Error::Io(e.error, "commit".into()))?;
        if !existed { stats.lock().unwrap().4.push(p.clone()); }
        apply_meta(&p, unix_mode(infos[idx].0, false), infos[idx].1, false);
        progress.add_written(n); progress.file_done();
        let mut s = stats.lock().unwrap(); s.0 += 1; s.2 += n;
        Ok(true)
    };
    let res: Result<()> = pool.install(|| {
        (0..blocks).into_par_iter().try_for_each(|bi| -> Result<()> {
            cancel.check()?;
            let mut src = File::open(path)?;
            let dec = sz::BlockDecoder::new(1, bi, &archive, &pw, &mut src);
            dec.for_each_entries(&mut |e: &sz::ArchiveEntry, r: &mut dyn Read| handle_entry(e.name(), r)).map(|_| ()).map_err(|e| if cancel.is_cancelled() { Error::new(ErrorKind::Cancelled, "cancelled") } else { sz_err(e) })
        })
    });
    if let Err(e) = res {
        if e.kind == ErrorKind::Cancelled && o.delete_partial_on_cancel { for p in &stats.lock().unwrap().4 { let _ = std::fs::remove_file(p); } }
        return Err(e);
    }
    // empty files
    for (i, t) in targets.iter().enumerate() {
        let f = &archive.files[i];
        if let Some(t) = t { if !f.is_directory() && !f.has_stream() {
            if let Some(p) = t.path.parent() { std::fs::create_dir_all(p)?; }
            if let Some(p) = resolve_conflict(&t.path, o, &ask) { File::create(&p)?; apply_meta(&p, unix_mode(infos[i].0, false), infos[i].1, false); stats.lock().unwrap().0 += 1; }
        } }
    }
    let mut dirs: Vec<(PathBuf, i64, u32)> = targets.iter().enumerate().filter(|(i, t)| t.is_some() && names[*i].1).map(|(i, t)| (t.as_ref().unwrap().path.clone(), infos[i].1, unix_mode(infos[i].0, true))).collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.0.components().count()));
    for (p, t, m) in dirs { apply_meta(&p, m, t, true); }
    let s = stats.into_inner().unwrap();
    report.extracted = s.0; report.skipped = s.1; report.bytes = s.2; report.failed = s.3;
    Ok(report)
}
