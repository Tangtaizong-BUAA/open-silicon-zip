//! Format detection and the unified open/list/extract/test entry points.
pub mod codecs;
pub mod common;
pub mod legacy;
pub mod sevenz;
pub mod tar;

use crate::cancel::{CancelToken, Progress};
use crate::error::{Error, ErrorKind, Result};
use crate::zip::extract::{ExtractOptions, ExtractReport};
use codecs::Codec;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format { Zip, SevenZ, Rar, Tar(Codec), Stream(Codec), Legacy, Unknown }

const SIG7Z: [u8; 6] = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];

fn scan_sfx(path: &Path) -> Option<Format> {
    let mut b = vec![0u8; 4 << 20];
    let n = File::open(path).ok()?.read(&mut b).ok()?;
    let b = &b[..n];
    if !b.starts_with(b"MZ") && !b.starts_with(&[0x7f, b'E', b'L', b'F']) { return None; }
    for i in 0..n.saturating_sub(8) {
        if &b[i..i + 4] == b"PK\x03\x04" { return Some(Format::Zip); }
        if b[i..i + 6] == SIG7Z { return Some(Format::SevenZ); }
        if &b[i..i + 6] == b"Rar!\x1a\x07" { return Some(Format::Rar); }
    }
    None
}

/// Content-first sniffing; extension is only a hint (03 §5).
pub fn detect(path: &Path) -> Result<Format> {
    let mut head = [0u8; 16];
    let n = File::open(path).map_err(Error::from)?.read(&mut head)?;
    let h = &head[..n];
    if h.starts_with(b"PK\x03\x04") || h.starts_with(b"PK\x05\x06") || h.starts_with(b"PK\x07\x08") { return Ok(Format::Zip); }
    if h.starts_with(&SIG7Z) { return Ok(Format::SevenZ); }
    if h.starts_with(b"Rar!\x1a\x07") { return Ok(Format::Rar); }
    if let Some(f) = scan_sfx(path) { return Ok(f); }
    // final volume of a spanned ZIP (or any ZIP with leading junk): end-of-central-directory near the tail
    {
        use std::io::{Seek, SeekFrom};
        let mut f = File::open(path)?;
        let len = f.metadata()?.len();
        let take = len.min(0x10000 + 22);
        f.seek(SeekFrom::Start(len - take))?;
        let mut t = vec![0u8; take as usize];
        f.read_exact(&mut t)?;
        if t.windows(4).rev().any(|w| w == b"PK\x05\x06") && t.len() >= 22 { return Ok(Format::Zip); }
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    // spanned ZIP volume: `.z01` holds data only (starts with the span marker), the `.zip` has the directory
    if ext.len() == 3 && ext.starts_with('z') && ext[1..].chars().all(|c| c.is_ascii_digit()) { return Ok(Format::Zip); }
    if let Some(c) = tar::sniff_codec(path).ok().filter(|c| *c != Codec::None || ext == "tar") {
        if tar::is_tar(path) { return Ok(Format::Tar(c)); }
        if c != Codec::None { return Ok(Format::Stream(c)); }
    }
    if tar::is_tar(path) { return Ok(Format::Tar(Codec::None)); }
    if ext == "br" || ext == "lzma" {
        let c = Codec::from_ext(&ext).unwrap();
        return Ok(if tar::is_tar(path) { Format::Tar(c) } else { Format::Stream(c) });
    }
    if legacy::probe(path) { return Ok(Format::Legacy); }
    Ok(Format::Unknown)
}

/// If `path` is `.7z.001` (or any `.NNN` set whose joined content is a known format), join into `tmp`.
pub fn resolve_generic_split(path: &Path, tmp: &Path) -> Result<Option<PathBuf>> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.len() == 3 && ext.chars().all(|c| c.is_ascii_digit()) {
        return Ok(Some(sevenz::join_volumes(path, tmp)?));
    }
    Ok(None)
}

#[derive(Debug, Clone)]
pub struct ListItem { pub name: String, pub size: u64, pub csize: u64, pub mtime: i64, pub is_dir: bool, pub method: String, pub encrypted: bool, pub crc: Option<u32>, pub attrs: u32 }

/// Run `f` on the joined file when `path` is a generic split volume (`.001`), else on `path` itself.
fn resolving<T>(path: &Path, f: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    let is_split = path.extension().and_then(|e| e.to_str()).map_or(false, |e| e.len() == 3 && e.chars().all(|c| c.is_ascii_digit()));
    if !is_split { return f(path); }
    let tmp = tempfile::Builder::new().prefix("fz-join-").tempdir()?;
    let joined = sevenz::join_volumes(path, tmp.path())?;
    f(&joined)
}

pub fn list(path: &Path, password: Option<&str>, codepage: Option<&str>, cancel: &CancelToken) -> Result<Vec<ListItem>> {
    resolving(path, |path| {
        let items = list_inner(path, password, codepage, cancel)?;
        let mut budget = crate::limits::ListingBudget::new(Default::default());
        for item in &items {
            cancel.check()?;
            budget.reserve(item.name.len())?;
            budget.reserve_json(&item.name)?;
        }
        Ok(items)
    })
}
pub fn extract(path: &Path, o: &ExtractOptions, codepage: Option<&str>, cancel: &CancelToken, progress: &Progress) -> Result<ExtractReport> {
    // smart-extract names the wrapper after the original volume set, not the temp join
    resolving(path, |p| extract_inner(p, o, codepage, cancel, progress))
}
pub fn test(path: &Path, password: Option<&str>, cancel: &CancelToken, progress: &Progress) -> Result<Vec<(String, Result<()>)>> {
    resolving(path, |p| test_inner(p, password, cancel, progress))
}

fn list_inner(path: &Path, password: Option<&str>, codepage: Option<&str>, cancel: &CancelToken) -> Result<Vec<ListItem>> {
    match detect(path)? {
        Format::Zip => {
            let mut ro = crate::zip::ReadOptions::default();
            ro.codepage = codepage.map(|s| s.to_string());
            let a = crate::zip::ZipArchive::open(path, ro)?;
            Ok(a.entries.iter().map(|e| ListItem { name: e.name.clone(), size: e.size, csize: e.csize, mtime: e.mtime, is_dir: e.is_dir, method: e.method.to_string(),
                encrypted: e.enc != crate::zip::reader::EncKind::None, crc: Some(e.crc), attrs: e.mode }).collect())
        }
        Format::SevenZ => Ok(sevenz::list_7z(path, password)?.into_iter().map(|e| ListItem { name: e.name, size: e.size, csize: e.csize, mtime: e.mtime, is_dir: e.is_dir,
            method: "7z".into(), encrypted: password.is_some(), crc: Some(e.crc), attrs: e.attrs }).collect()),
        Format::Tar(_) => Ok(tar::list_tar(path, crate::sys::default_memory_budget(), cancel)?.into_iter().map(|e| ListItem { name: e.name, size: e.size, csize: 0, mtime: e.mtime,
            is_dir: e.is_dir, method: "tar".into(), encrypted: false, crc: None, attrs: e.mode }).collect()),
        Format::Stream(_) => {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("data").to_string();
            Ok(vec![ListItem { name: stem, size: 0, csize: std::fs::metadata(path)?.len(), mtime: 0, is_dir: false, method: "stream".into(), encrypted: false, crc: None, attrs: 0o644 }])
        }
        Format::Rar => rar_list(path, password),
        Format::Legacy => legacy::list_legacy(path, password, cancel),
        Format::Unknown => Err(Error::new(ErrorKind::UnsupportedFormat, format!("{}: unknown file format", path.display()))),
    }
}

fn extract_inner(path: &Path, o: &ExtractOptions, codepage: Option<&str>, cancel: &CancelToken, progress: &Progress) -> Result<ExtractReport> {
    match detect(path)? {
        Format::Zip => {
            let mut ro = crate::zip::ReadOptions::default();
            ro.codepage = codepage.map(|s| s.to_string());
            let a = crate::zip::ZipArchive::open(path, ro)?;
            crate::zip::extract(&a, o, cancel, progress)
        }
        Format::SevenZ => sevenz::extract_7z(path, o, cancel, progress),
        Format::Tar(_) => tar::extract_tar(path, o, cancel, progress),
        Format::Stream(c) => {
            let t = tar::extract_single_stream(path, &o.dest, c, cancel, progress)?;
            Ok(ExtractReport { extracted: 1, bytes: std::fs::metadata(&t)?.len(), root: o.dest.clone(), ..Default::default() })
        }
        Format::Rar => rar_extract(path, o, cancel, progress),
        Format::Legacy => legacy::extract_legacy(path, o, cancel, progress),
        Format::Unknown => Err(Error::new(ErrorKind::UnsupportedFormat, format!("{}: unknown file format", path.display()))),
    }
}

/// Verify every entry (FR-701). Returns (name, result) per entry.
fn test_inner(path: &Path, password: Option<&str>, cancel: &CancelToken, progress: &Progress) -> Result<Vec<(String, Result<()>)>> {
    match detect(path)? {
        Format::Zip => {
            let a = crate::zip::ZipArchive::open(path, crate::zip::ReadOptions::default())?;
            crate::zip::test_archive(&a, password, cancel, progress)
        }
        Format::SevenZ => {
            use sevenz_rust2 as sz;
            let arc = sevenz::open_archive(path, password)?;
            let pw = password.map(sz::Password::from).unwrap_or_else(sz::Password::empty);
            let mut res = Vec::new();
            for bi in 0..arc.blocks.len() {
                cancel.check()?;
                let mut src = File::open(path)?;
                let dec = sz::BlockDecoder::new(1, bi, &arc, &pw, &mut src);
                let r = dec.for_each_entries(&mut |e: &sz::ArchiveEntry, r: &mut dyn Read| {
                    let rr = std::io::copy(r, &mut std::io::sink());
                    res.push((e.name().to_string(), rr.map(|_| ()).map_err(|er| Error::new(ErrorKind::CrcMismatch, er.to_string()))));
                    Ok(true)
                });
                if let Err(e) = r { res.push((format!("block {bi}"), Err(sevenz::map_err(e)))); }
            }
            Ok(res)
        }
        Format::Tar(_) => {
            let r = tar::list_tar(path, crate::sys::default_memory_budget(), cancel);
            Ok(match r { Ok(v) => v.into_iter().map(|e| (e.name, Ok(()))).collect(), Err(e) => vec![(path.display().to_string(), Err(e))] })
        }
        Format::Stream(c) => {
            let mut r = codecs::decompressor(c, File::open(path)?, crate::sys::default_memory_budget())?;
            let res = std::io::copy(&mut r, &mut std::io::sink()).map(|_| ()).map_err(codecs::map_decode_err);
            Ok(vec![(path.display().to_string(), res)])
        }
        Format::Rar => rar_test(path, password),
        Format::Legacy => legacy::test_legacy(path, password, cancel, progress),
        Format::Unknown => Err(Error::new(ErrorKind::UnsupportedFormat, "unknown file format")),
    }
}

// ---- RAR (extraction only, via UnRAR)
fn rar_err(e: unrar::error::UnrarError) -> Error {
    let s = e.to_string();
    let l = s.to_lowercase();
    if l.contains("password") { Error::new(ErrorKind::PasswordRequired, s) }
    else if l.contains("crc") { Error::new(ErrorKind::CrcMismatch, s) }
    else if l.contains("volume") { Error::new(ErrorKind::MissingVolume, s) }
    else { Error::new(ErrorKind::BrokenArchive, s) }
}

fn rar_list(path: &Path, password: Option<&str>) -> Result<Vec<ListItem>> {
    let ar = match password { Some(p) => unrar::Archive::with_password(path, p), None => unrar::Archive::new(path) };
    let mut out = Vec::new();
    for h in ar.open_for_listing().map_err(rar_err)? {
        let h = h.map_err(rar_err)?;
        out.push(ListItem { name: h.filename.to_string_lossy().replace('\\', "/"), size: h.unpacked_size as u64, csize: 0, mtime: 0, is_dir: h.is_directory(), method: "rar".into(),
            encrypted: h.is_encrypted(), crc: Some(h.file_crc), attrs: 0o644 });
    }
    Ok(out)
}

fn rar_extract(path: &Path, o: &ExtractOptions, cancel: &CancelToken, progress: &Progress) -> Result<ExtractReport> {
    let items = rar_list(path, o.password.as_deref())?;
    let mut report = ExtractReport::default();
    let names: Vec<(String, bool)> = items.iter().map(|i| (i.name.clone(), i.is_dir)).collect();
    let (_d, targets) = common::plan_targets(&names, path, o, &mut report)?;
    progress.set_totals(items.len() as u64, items.iter().map(|i| i.size).sum());
    let ar = match &o.password { Some(p) => unrar::Archive::with_password(path, p), None => unrar::Archive::new(path) };
    let mut cur = ar.open_for_processing().map_err(rar_err)?;
    let ask = |p: &Path| o.on_conflict.as_ref().map(|f| f(p, None)).unwrap_or(crate::zip::ConflictChoice::Skip);
    let mut idx = 0usize;
    while let Some(h) = cur.read_header().map_err(rar_err)? {
        cancel.check()?;
        let t = targets.get(idx).cloned().flatten();
        idx += 1;
        match t {
            Some(t) if !h.entry().is_directory() => {
                if let Some(p) = t.path.parent() { std::fs::create_dir_all(p)?; }
                match common::resolve_conflict(&t.path, o, &ask) {
                    Some(dst) => { cur = h.extract_to(&dst).map_err(rar_err)?; report.extracted += 1; progress.file_done(); }
                    None => { cur = h.skip().map_err(rar_err)?; report.skipped += 1; }
                }
            }
            Some(t) => { std::fs::create_dir_all(&t.path)?; cur = h.skip().map_err(rar_err)?; }
            None => { cur = h.skip().map_err(rar_err)?; }
        }
    }
    Ok(report)
}

fn rar_test(path: &Path, password: Option<&str>) -> Result<Vec<(String, Result<()>)>> {
    let ar = match password { Some(p) => unrar::Archive::with_password(path, p), None => unrar::Archive::new(path) };
    let mut cur = ar.open_for_processing().map_err(rar_err)?;
    let mut out = Vec::new();
    loop {
        match cur.read_header() {
            Ok(Some(h)) => {
                let name = h.entry().filename.to_string_lossy().into_owned();
                match h.test() { Ok(c) => { out.push((name, Ok(()))); cur = c; } Err(e) => { out.push((name, Err(rar_err(e)))); break; } }
            }
            Ok(None) => break,
            Err(e) => { out.push((path.display().to_string(), Err(rar_err(e)))); break; }
        }
    }
    Ok(out)
}
