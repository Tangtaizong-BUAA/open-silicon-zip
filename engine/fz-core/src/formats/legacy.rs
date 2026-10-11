//! Legacy read-side breadth (spec 03 §1) through the system libarchive, plus a native
//! UUE / XXE / Base64 text decoder.
//!
//! libarchive is linked dynamically (`-larchive`; on macOS it ships in the SDK, on Linux install
//! `libarchive-dev`), through a tiny hand-written FFI surface so no vendored build is needed.
//! Without the `legacy` cargo feature every entry point returns `UnsupportedFormat`.
//!
//! Formats offered to libarchive: CAB, LHA/LZH (+SFX), ISO 9660, cpio, ar (.a/.deb), xar, WARC, mtree,
//! tar (for tar.lz / tar.Z / tar.lrz ...), and single-stream "raw" for every libarchive filter
//! (lzip, Unix compress, uuencode, lrzip, grzip, lzop ...). RPM headers are stripped by libarchive's RPM
//! filter so the cpio payload is listed. A `.deb` lists/extracts its `data.tar.*` payload.
//! Not provided by libarchive: ARJ, ACE, ALZ, EGG, ZPAQ, PEA, BH, ARC/PMA, UDF/WIM (WIM only on libarchive >= 3.8).
//!
//! Safety: names go through `common::plan_targets` (zip-slip, junk filter, smart extract), symlink
//! targets through `pathsafe::symlink_target_safe`, no device/fifo/socket nodes are created, setuid/sticky
//! bits are never applied, symlinked ancestors are refused, and data is streamed in 256 KB chunks.

use super::ListItem;
use crate::cancel::{CancelToken, Progress};
use crate::error::Result;
use crate::zip::extract::{ExtractOptions, ExtractReport};
use std::path::Path;

pub const CHUNK: usize = 256 << 10;

#[cfg(feature = "legacy")]
pub use imp::{extract_legacy, list_legacy, probe, test_legacy};

#[cfg(not(feature = "legacy"))]
mod stub {
    use super::*;
    use crate::error::{Error, ErrorKind};
    fn off() -> Error { Error::new(ErrorKind::UnsupportedFormat, "legacy archive formats are not available: fz-core was built without the `legacy` feature (libarchive)") }
    pub fn probe(_p: &Path) -> bool { false }
    pub fn list_legacy(_p: &Path, _pw: Option<&str>, _c: &CancelToken) -> Result<Vec<ListItem>> { Err(off()) }
    pub fn extract_legacy(_p: &Path, _o: &ExtractOptions, _c: &CancelToken, _pr: &Progress) -> Result<ExtractReport> { Err(off()) }
    pub fn test_legacy(_p: &Path, _pw: Option<&str>, _c: &CancelToken, _pr: &Progress) -> Result<Vec<(String, Result<()>)>> { Err(off()) }
}
#[cfg(not(feature = "legacy"))]
pub use stub::{extract_legacy, list_legacy, probe, test_legacy};

#[cfg(feature = "legacy")]
mod ffi {
    use std::ffi::{c_char, c_int, c_void};
    #[repr(C)] pub struct Archive { _p: [u8; 0] }
    #[repr(C)] pub struct Entry { _p: [u8; 0] }
    #[cfg(target_os = "macos")] pub type ModeT = u16;
    #[cfg(not(target_os = "macos"))] pub type ModeT = u32;
    pub const OK: c_int = 0;
    pub const EOF: c_int = 1;
    pub const WARN: c_int = -20;
    pub const FMT_CPIO: c_int = 0x10000;
    pub const FMT_TAR: c_int = 0x30000;
    pub const FMT_ISO9660: c_int = 0x40000;
    pub const FMT_AR: c_int = 0x70000;
    pub const FMT_MTREE: c_int = 0x80000;
    pub const FMT_RAW: c_int = 0x90000;
    pub const FMT_XAR: c_int = 0xA0000;
    pub const FMT_LHA: c_int = 0xB0000;
    pub const FMT_CAB: c_int = 0xC0000;
    pub const FMT_WARC: c_int = 0xF0000;
    #[link(name = "archive")]
    unsafe extern "C" {
        pub fn archive_read_new() -> *mut Archive;
        pub fn archive_read_free(a: *mut Archive) -> c_int;
        pub fn archive_read_support_filter_all(a: *mut Archive) -> c_int;
        pub fn archive_read_support_format_by_code(a: *mut Archive, code: c_int) -> c_int;
        pub fn archive_read_add_passphrase(a: *mut Archive, p: *const c_char) -> c_int;
        pub fn archive_read_open_filename(a: *mut Archive, f: *const c_char, block: usize) -> c_int;
        pub fn archive_read_next_header(a: *mut Archive, e: *mut *mut Entry) -> c_int;
        pub fn archive_read_data_block(a: *mut Archive, buf: *mut *const c_void, n: *mut usize, off: *mut i64) -> c_int;
        pub fn archive_error_string(a: *mut Archive) -> *const c_char;
        pub fn archive_format(a: *mut Archive) -> c_int;
        pub fn archive_format_name(a: *mut Archive) -> *const c_char;
        pub fn archive_filter_count(a: *mut Archive) -> c_int;
        pub fn archive_filter_code(a: *mut Archive, n: c_int) -> c_int;
        pub fn archive_entry_pathname(e: *mut Entry) -> *const c_char;
        pub fn archive_entry_pathname_utf8(e: *mut Entry) -> *const c_char;
        pub fn archive_entry_symlink(e: *mut Entry) -> *const c_char;
        pub fn archive_entry_symlink_utf8(e: *mut Entry) -> *const c_char;
        pub fn archive_entry_hardlink(e: *mut Entry) -> *const c_char;
        pub fn archive_entry_hardlink_utf8(e: *mut Entry) -> *const c_char;
        pub fn archive_entry_size(e: *mut Entry) -> i64;
        pub fn archive_entry_size_is_set(e: *mut Entry) -> c_int;
        pub fn archive_entry_mtime(e: *mut Entry) -> i64;
        pub fn archive_entry_mtime_is_set(e: *mut Entry) -> c_int;
        pub fn archive_entry_mode(e: *mut Entry) -> ModeT;
        pub fn archive_entry_filetype(e: *mut Entry) -> ModeT;
        pub fn archive_entry_is_encrypted(e: *mut Entry) -> c_int;
    }
}

#[cfg(feature = "legacy")]
mod imp {
    use super::ffi::*;
    use super::*;
    use crate::error::{Error, ErrorKind};
    use crate::formats::common::{apply_meta, plan_targets, resolve_conflict};
    use crate::pathsafe;
    use crate::zip::extract::ConflictChoice;
    use std::collections::HashMap;
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::fs::File;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    // ------------------------------------------------------------------ libarchive wrapper
    #[derive(Debug, Clone)]
    pub struct EntryInfo {
        pub name: String,
        pub size: u64,
        pub mtime: i64,
        pub mode: u32,
        pub ftype: u32,
        pub symlink: Option<String>,
        pub hardlink: Option<String>,
        pub encrypted: bool,
    }
    const S_IFMT: u32 = 0o170000;
    const S_IFREG: u32 = 0o100000;
    const S_IFDIR: u32 = 0o040000;
    const S_IFLNK: u32 = 0o120000;

    impl EntryInfo {
        fn is_dir(&self) -> bool { self.ftype == S_IFDIR }
        fn is_link(&self) -> bool { self.ftype == S_IFLNK && self.symlink.is_some() }
        fn is_file(&self) -> bool { self.ftype == S_IFREG || (self.ftype == 0 && !self.name.ends_with('/')) }
    }

    pub struct Reader {
        a: *mut Archive, fmt_code: i32, filters: i32, fmt_name: String,
        // per-entry data cursor (see `read`)
        pos: u64, base: Option<i64>, limit: u64, pend: *const u8, pend_len: usize, gap: u64,
    }
    impl Drop for Reader { fn drop(&mut self) { unsafe { archive_read_free(self.a); } } }

    unsafe fn cstr(p: *const c_char) -> Option<String> {
        if p.is_null() { None } else { Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()) }
    }

    fn err_string(a: *mut Archive) -> String {
        unsafe { cstr(archive_error_string(a)) }.unwrap_or_else(|| "unknown libarchive error".into())
    }

    fn map_msg(msg: &str) -> Error {
        let l = msg.to_lowercase();
        let kind = if l.contains("passphrase required") || l.contains("password required") { ErrorKind::PasswordRequired }
            else if l.contains("incorrect passphrase") || l.contains("wrong pass") || l.contains("bad password") { ErrorKind::WrongPassword }
            else if l.contains("crc") || l.contains("checksum") || l.contains("check sum") { ErrorKind::CrcMismatch }
            else if l.contains("unrecognized archive format") || l.contains("unsupported archive format") { ErrorKind::UnsupportedFormat }
            else if l.contains("encrypt") && (l.contains("unsupported") || l.contains("not supported")) { ErrorKind::UnsupportedEncryption }
            else if l.contains("unsupported") || l.contains("not supported") || l.contains("unknown compression") || l.contains("unsupported compression") || l.contains("not built") { ErrorKind::UnsupportedMethod }
            else { ErrorKind::BrokenArchive };
        Error::new(kind, msg.to_string())
    }

    fn cpath(p: &Path) -> Result<CString> {
        #[cfg(unix)] let b = { use std::os::unix::ffi::OsStrExt; p.as_os_str().as_bytes().to_vec() };
        #[cfg(not(unix))] let b = p.to_string_lossy().into_owned().into_bytes();
        CString::new(b).map_err(|_| Error::new(ErrorKind::InvalidInput, "path contains NUL"))
    }

    impl Reader {
        pub fn open(path: &Path, password: Option<&str>) -> Result<Reader> {
            let a = unsafe { archive_read_new() };
            if a.is_null() { return Err(Error::new(ErrorKind::Io, "libarchive: out of memory")); }
            let mut r = Reader { a, fmt_code: 0, filters: 0, fmt_name: String::new(), pos: 0, base: None, limit: u64::MAX, pend: std::ptr::null(), pend_len: 0, gap: 0 };
            unsafe {
                archive_read_support_filter_all(a);
                // content formats; zip/7z/rar/tar-family are owned by the dedicated readers. tar stays enabled for tar.lz / tar.Z.
                for c in [FMT_CPIO, FMT_ISO9660, FMT_AR, FMT_XAR, FMT_LHA, FMT_CAB, FMT_WARC, FMT_MTREE, FMT_TAR, FMT_RAW] {
                    archive_read_support_format_by_code(a, c); // an unavailable format just stays unregistered
                }
                if let Some(p) = password {
                    let c = CString::new(p).map_err(|_| Error::new(ErrorKind::InvalidInput, "password contains NUL"))?;
                    archive_read_add_passphrase(a, c.as_ptr());
                }
                let cp = cpath(path)?;
                if archive_read_open_filename(a, cp.as_ptr(), CHUNK) != OK {
                    let m = err_string(a);
                    return Err(if std::fs::metadata(path).is_err() { Error::new(ErrorKind::Io, format!("{}: {m}", path.display())) } else { map_msg(&m) });
                }
            }
            r.fmt_code = 0;
            Ok(r)
        }

        fn refresh(&mut self) {
            unsafe {
                self.fmt_code = archive_format(self.a) & !0xffff;
                self.fmt_name = cstr(archive_format_name(self.a)).unwrap_or_default();
                let n = archive_filter_count(self.a);
                self.filters = (0..n).filter(|i| archive_filter_code(self.a, *i) != 0).count() as i32;
            }
        }

        /// Next header; `Ok(None)` at end of archive.
        pub fn next(&mut self, raw_name: &str) -> Result<Option<EntryInfo>> {
            let mut e: *mut Entry = std::ptr::null_mut();
            let rc = unsafe { archive_read_next_header(self.a, &mut e) };
            if rc == EOF { return Ok(None); }
            if rc != OK && rc != WARN { return Err(map_msg(&err_string(self.a))); }
            self.refresh();
            self.pos = 0; self.base = None; self.pend = std::ptr::null(); self.pend_len = 0; self.gap = 0;
            self.limit = u64::MAX;
            if self.fmt_code == FMT_RAW && self.filters == 0 {
                return Err(Error::new(ErrorKind::UnsupportedFormat, "not a recognised archive (no known container or compression filter)"));
            }
            unsafe {
                let name = cstr(archive_entry_pathname_utf8(e)).or_else(|| cstr(archive_entry_pathname(e))).unwrap_or_default();
                let name = if self.fmt_code == FMT_RAW { raw_name.to_string() } else { name };
                let sl = cstr(archive_entry_symlink_utf8(e)).or_else(|| cstr(archive_entry_symlink(e)));
                let hl = cstr(archive_entry_hardlink_utf8(e)).or_else(|| cstr(archive_entry_hardlink(e)));
                if archive_entry_size_is_set(e) != 0 && self.fmt_code != FMT_RAW { self.limit = archive_entry_size(e).max(0) as u64; }
                let mut ftype = archive_entry_filetype(e) as u32 & S_IFMT;
                if self.fmt_code == FMT_RAW { ftype = S_IFREG; }
                Ok(Some(EntryInfo {
                    name,
                    size: if archive_entry_size_is_set(e) != 0 { archive_entry_size(e).max(0) as u64 } else { 0 },
                    mtime: if archive_entry_mtime_is_set(e) != 0 { archive_entry_mtime(e) } else { 0 },
                    mode: archive_entry_mode(e) as u32 & 0o7777,
                    ftype, symlink: sl, hardlink: hl,
                    encrypted: archive_entry_is_encrypted(e) != 0,
                }))
            }
        }

        /// Stream the current entry's data. Built on `archive_read_data_block` so holes (sparse entries) become zeros
        /// in bounded pieces; xar reports offsets that continue across entries, so those are rebased per entry.
        pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            loop {
                if self.gap > 0 {
                    let n = (self.gap as usize).min(buf.len());
                    buf[..n].fill(0);
                    self.gap -= n as u64; self.pos += n as u64;
                    return Ok(n);
                }
                if self.pend_len > 0 {
                    let n = self.pend_len.min(buf.len());
                    unsafe { std::ptr::copy_nonoverlapping(self.pend, buf.as_mut_ptr(), n); self.pend = self.pend.add(n); }
                    self.pend_len -= n; self.pos += n as u64;
                    return Ok(n);
                }
                let mut p: *const c_void = std::ptr::null();
                let (mut sz, mut off) = (0usize, 0i64);
                let rc = unsafe { archive_read_data_block(self.a, &mut p, &mut sz, &mut off) };
                if rc == EOF { return Ok(0); }
                // a data-time WARN is how libarchive reports e.g. a failed per-file CRC: treat it as damage
                if rc != OK { return Err(map_msg(&err_string(self.a))); }
                if self.fmt_code == FMT_XAR { let b = *self.base.get_or_insert(off); off -= b; }
                if off < 0 || (off as u64) < self.pos { return Err(Error::new(ErrorKind::BrokenArchive, "data block offset moves backwards")); }
                if off as u64 > self.limit { return Err(Error::new(ErrorKind::BrokenArchive, "data block offset beyond entry size")); }
                self.gap = off as u64 - self.pos;
                self.pend = p as *const u8; self.pend_len = if p.is_null() { 0 } else { sz };
            }
        }
    }

    fn raw_name_for(path: &Path) -> String {
        let n = path.file_name().and_then(|s| s.to_str()).unwrap_or("data");
        match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) if !s.is_empty() && path.extension().is_some() => s.to_string(),
            _ => format!("{n}.out"),
        }
    }

    // ------------------------------------------------------------------ detection
    /// True when libarchive recognises `path` (or the text decoder does). Never reads data beyond the first header.
    pub fn probe(path: &Path) -> bool {
        if sniff_text(path).is_some() { return true; }
        let Ok(mut r) = Reader::open(path, None) else { return false };
        matches!(r.next("x"), Ok(Some(_)))
    }

    // ------------------------------------------------------------------ deb: nested data.tar.* payload
    struct TmpDir(PathBuf);
    impl Drop for TmpDir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// The file libarchive should walk: `path` itself, or the `data.tar.*` member of a Debian package.
    struct Source { path: PathBuf, _tmp: Option<TmpDir> }

    fn resolve_source(path: &Path, cancel: &CancelToken) -> Result<Source> {
        let plain = || Source { path: path.to_path_buf(), _tmp: None };
        let mut r = match Reader::open(path, None) { Ok(r) => r, Err(_) => return Ok(plain()) };
        let rn = raw_name_for(path);
        let first = match r.next(&rn) { Ok(Some(e)) => e, _ => return Ok(plain()) };
        if r.fmt_code != FMT_AR { return Ok(plain()); }
        let mut cur = Some(first);
        while let Some(e) = cur {
            cancel.check()?;
            if e.name.starts_with("data.tar") {
                let dir = std::env::temp_dir().join(format!("fz-legacy-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
                std::fs::create_dir_all(&dir)?;
                let guard = TmpDir(dir.clone());
                let out = dir.join(&e.name);
                let mut w = std::io::BufWriter::new(File::create(&out)?);
                let mut buf = vec![0u8; CHUNK];
                loop {
                    cancel.check()?;
                    let n = r.read(&mut buf)?;
                    if n == 0 { break; }
                    w.write_all(&buf[..n])?;
                }
                w.flush()?;
                return Ok(Source { path: out, _tmp: Some(guard) });
            }
            cur = r.next(&rn)?;
        }
        Ok(plain())
    }

    fn collect(path: &Path, password: Option<&str>, cancel: &CancelToken) -> Result<(Vec<EntryInfo>, String)> {
        let mut r = Reader::open(path, password)?;
        let rn = raw_name_for(path);
        let mut v = Vec::new();
        while let Some(mut e) = r.next(&rn)? {
            cancel.check()?;
            e.name = clean_name(&e.name);
            v.push(e);
        }
        Ok((v, r.fmt_name.clone()))
    }

    fn clean_name(n: &str) -> String {
        let mut s = n.replace('\\', "/");
        while let Some(r) = s.strip_prefix("./") { s = r.to_string(); }
        if s == "." { s.clear(); }
        s
    }

    // ------------------------------------------------------------------ list
    pub fn list_legacy(path: &Path, password: Option<&str>, cancel: &CancelToken) -> Result<Vec<ListItem>> {
        if let Some(t) = sniff_text(path) {
            let mut n = 0u64;
            let info = decode_text(path, &t, &mut CountSink(&mut n), cancel)?;
            return Ok(vec![ListItem { name: info.name, size: n, csize: std::fs::metadata(path)?.len(), mtime: 0, is_dir: false, method: t.kind.label().into(), encrypted: false, crc: None, attrs: info.mode }]);
        }
        let src = resolve_source(path, cancel)?;
        let (items, fmt) = collect(&src.path, password, cancel)?;
        let csize = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let single = items.len() == 1;
        Ok(items.into_iter().map(|e| ListItem {
            is_dir: e.is_dir(), size: e.size, csize: if single { csize } else { 0 }, mtime: e.mtime, method: fmt.clone(), encrypted: e.encrypted, crc: None,
            attrs: e.mode | e.ftype, name: e.name,
        }).collect())
    }

    // ------------------------------------------------------------------ test
    pub fn test_legacy(path: &Path, password: Option<&str>, cancel: &CancelToken, progress: &Progress) -> Result<Vec<(String, Result<()>)>> {
        if let Some(t) = sniff_text(path) {
            let mut n = 0u64;
            let r = decode_text(path, &t, &mut CountSink(&mut n), cancel);
            return match r {
                Ok(i) => Ok(vec![(i.name, Ok(()))]),
                Err(e) if e.kind == ErrorKind::Cancelled => Err(e),
                Err(e) => Ok(vec![(path.display().to_string(), Err(e))]),
            };
        }
        let src = resolve_source(path, cancel)?;
        let mut r = Reader::open(&src.path, password)?;
        let rn = raw_name_for(path);
        let mut out = Vec::new();
        let mut buf = vec![0u8; CHUNK];
        loop {
            cancel.check()?;
            let e = match r.next(&rn) {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(e) => { out.push((path.display().to_string(), Err(e))); break; }
            };
            let name = clean_name(&e.name);
            progress.set_current(&name);
            let mut res: Result<()> = Ok(());
            if e.encrypted && password.is_none() { res = Err(Error::new(ErrorKind::PasswordRequired, "entry is encrypted")); }
            else {
                let mut total = 0u64;
                loop {
                    cancel.check()?;
                    match r.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => { total += n as u64; progress.add_read(n as u64); }
                        Err(er) => { res = Err(er); break; }
                    }
                }
                if res.is_ok() && e.is_file() && e.size > 0 && total != e.size && e.hardlink.is_none() {
                    res = Err(Error::new(ErrorKind::BrokenArchive, format!("size mismatch: expected {} bytes, read {total}", e.size)));
                }
            }
            if !name.is_empty() || res.is_err() { out.push((if name.is_empty() { ".".into() } else { name }, res)); }
            progress.file_done();
        }
        Ok(out)
    }

    // ------------------------------------------------------------------ extract
    fn has_symlink_ancestor(dest: &Path, target: &Path) -> bool {
        let Ok(rel) = target.strip_prefix(dest) else { return true };
        let mut cur = dest.to_path_buf();
        let comps: Vec<_> = rel.components().collect();
        for c in comps.iter().take(comps.len().saturating_sub(1)) {
            cur.push(c);
            if let Ok(m) = std::fs::symlink_metadata(&cur) { if m.file_type().is_symlink() { return true; } }
        }
        false
    }

    pub fn extract_legacy(path: &Path, o: &ExtractOptions, cancel: &CancelToken, progress: &Progress) -> Result<ExtractReport> {
        let mut report = ExtractReport::default();
        if let Some(t) = sniff_text(path) { return extract_text(path, &t, o, cancel, progress, report); }
        let src = resolve_source(path, cancel)?;
        let (items, _) = collect(&src.path, o.password.as_deref(), cancel)?;
        let names: Vec<(String, bool)> = items.iter().map(|i| (i.name.clone(), i.is_dir())).collect();
        let (dest, targets) = plan_targets(&names, path, o, &mut report)?;
        progress.set_totals(items.len() as u64, items.iter().map(|i| i.size).sum());
        progress.set_phase("extracting");
        let ask = |p: &Path| o.on_conflict.as_ref().map(|f| f(p, None)).unwrap_or(ConflictChoice::Skip);
        let mut r = Reader::open(&src.path, o.password.as_deref())?;
        let rn = raw_name_for(path);
        let mut created: Vec<PathBuf> = Vec::new();
        let mut dirs: Vec<(PathBuf, u32, i64)> = Vec::new();
        let mut written: HashMap<String, PathBuf> = HashMap::new();
        let mut hardlinks: Vec<(PathBuf, String, String)> = Vec::new();
        let mut buf = vec![0u8; CHUNK];
        let res: Result<()> = (|| {
            let mut idx = 0usize;
            while let Some(e) = r.next(&rn)? {
                cancel.check()?;
                let i = idx; idx += 1;
                let Some(Some(t)) = targets.get(i) else { continue };
                let info = &items[i];
                progress.set_current(&t.rel);
                if has_symlink_ancestor(&dest, &t.path) { report.unsafe_refused.push(info.name.clone()); continue; }
                if e.is_dir() {
                    std::fs::create_dir_all(&t.path)?;
                    dirs.push((t.path.clone(), info.mode, info.mtime));
                    continue;
                }
                if e.is_link() {
                    let target = e.symlink.clone().unwrap_or_default();
                    let parent_rel = Path::new(&t.rel).parent().unwrap_or(Path::new("")).to_path_buf();
                    if !o.allow_external_symlinks && !pathsafe::symlink_target_safe(&dest, &parent_rel, &target) {
                        report.unsafe_refused.push(format!("{} -> {}", info.name, target));
                        continue;
                    }
                    if let Some(p) = t.path.parent() { std::fs::create_dir_all(p)?; }
                    match resolve_conflict(&t.path, o, &ask) {
                        Some(p) => { let _ = std::fs::remove_file(&p); make_symlink(&target, &p)?; report.extracted += 1; }
                        None => report.skipped += 1,
                    }
                    progress.file_done();
                    continue;
                }
                if let Some(h) = &e.hardlink {
                    if e.size == 0 { hardlinks.push((t.path.clone(), clean_name(h), info.name.clone())); continue; }
                }
                if !e.is_file() { report.skipped += 1; continue; } // device nodes, fifos, sockets: never created
                if let Some(p) = t.path.parent() { std::fs::create_dir_all(p)?; }
                let Some(p) = resolve_conflict(&t.path, o, &ask) else { report.skipped += 1; progress.file_done(); continue };
                if has_symlink_ancestor(&dest, &p) { report.unsafe_refused.push(info.name.clone()); continue; }
                let existed = p.exists();
                let temporary = tempfile::NamedTempFile::new_in(p.parent().unwrap_or(&dest))?;
                let mut w = std::io::BufWriter::with_capacity(CHUNK, temporary.as_file());
                let mut n = 0u64;
                let mut failure: Option<Error> = None;
                loop {
                    cancel.check()?;
                    match r.read(&mut buf) {
                        Ok(0) => break,
                        Ok(k) => { w.write_all(&buf[..k])?; n += k as u64; progress.add_written(k as u64); }
                        Err(er) => { failure = Some(er); break; }
                    }
                }
                if let Some(er) = failure {
                    drop(w);

                    if matches!(er.kind, ErrorKind::PasswordRequired | ErrorKind::WrongPassword) { return Err(er); }
                    report.failed.push((info.name.clone(), er.detail));
                    continue;
                }
                w.flush()?; drop(w);
                temporary.persist(&p).map_err(|e| Error::from(e.error))?;
                if !existed { created.push(p.clone()); }
                apply_meta(&p, info.mode, info.mtime, false);
                // newc/odc hard-link sets store the data on the LAST name; libarchive reports it with the first name as `hardlink`.
                // Make the earlier (empty) file share this content.
                if let Some(h) = e.hardlink.as_deref().map(clean_name) {
                    if let Some(tp) = written.get(&h).cloned() {
                        if std::fs::metadata(&tp).map(|m| m.len() == 0).unwrap_or(false) {
                            let _ = std::fs::remove_file(&tp);
                            if std::fs::hard_link(&p, &tp).is_err() { let _ = std::fs::copy(&p, &tp); }
                        }
                    }
                }
                written.insert(info.name.clone(), p);
                progress.file_done();
                report.extracted += 1; report.bytes += n;
            }
            Ok(())
        })();
        if let Err(e) = res {
            if e.kind == ErrorKind::Cancelled && o.delete_partial_on_cancel { for p in &created { let _ = std::fs::remove_file(p); } }
            return Err(e);
        }
        for (p, link, name) in hardlinks {
            let Some(src_p) = written.get(&link) else { report.failed.push((name, format!("hard link target {link:?} was not extracted"))); continue };
            if let Some(q) = resolve_conflict(&p, o, &ask) {
                let _ = std::fs::remove_file(&q);
                if std::fs::hard_link(src_p, &q).is_ok() || std::fs::copy(src_p, &q).is_ok() { report.extracted += 1; progress.file_done(); }
                else { report.failed.push((name, "could not create hard link".into())); }
            } else { report.skipped += 1; }
        }
        dirs.sort_by_key(|d| std::cmp::Reverse(d.0.components().count()));
        for (p, m, t) in dirs { apply_meta(&p, m, t, true); }
        Ok(report)
    }

    #[cfg(unix)]
    fn make_symlink(target: &str, p: &Path) -> Result<()> { std::os::unix::fs::symlink(target, p).map_err(Error::from) }
    #[cfg(not(unix))]
    fn make_symlink(_t: &str, _p: &Path) -> Result<()> { Err(Error::new(ErrorKind::UnsupportedFormat, "symlinks unsupported on this platform")) }

    // ------------------------------------------------------------------ UUE / XXE / Base64
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum TextKind { Uu, Xx, B64 }
    impl TextKind { fn label(self) -> &'static str { match self { TextKind::Uu => "uuencode", TextKind::Xx => "xxencode", TextKind::B64 => "base64" } } }
    struct TextHdr { kind: TextKind, name: String, mode: u32, skip_lines: usize }
    struct TextInfo { name: String, mode: u32 }

    struct CountSink<'a>(&'a mut u64);
    impl Write for CountSink<'_> {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { *self.0 += b.len() as u64; Ok(b.len()) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    const XX: &[u8; 64] = b"+-0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    fn xx_val(c: u8) -> Option<u8> { XX.iter().position(|x| *x == c).map(|p| p as u8) }
    fn uu_val(c: u8) -> Option<u8> { if (b' '..=b'`').contains(&c) { Some((c - b' ') & 63) } else { None } }
    fn b64_val(c: u8) -> Option<u8> {
        match c { b'A'..=b'Z' => Some(c - b'A'), b'a'..=b'z' => Some(c - b'a' + 26), b'0'..=b'9' => Some(c - b'0' + 52), b'+' => Some(62), b'/' => Some(63), _ => None }
    }

    fn sniff_text(path: &Path) -> Option<TextHdr> {
        let f = File::open(path).ok()?;
        let mut rd = BufReader::new(f.take(256 << 10));
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let mut line = Vec::new();
        let mut ln = 0usize;
        let mut hdr: Option<(TextKind, String, u32, usize)> = None;
        let mut lower_seen = false;
        let mut body = 0;
        while ln < 4000 {
            line.clear();
            if rd.read_until(b'\n', &mut line).ok()? == 0 { break; }
            ln += 1;
            let s = String::from_utf8_lossy(&line);
            let s = s.trim_end_matches(['\r', '\n']);
            if hdr.is_none() {
                let (kind, rest) = if let Some(r) = s.strip_prefix("begin-base64 ") { (TextKind::B64, r) }
                    else if let Some(r) = s.strip_prefix("begin ") { (TextKind::Uu, r) } else { continue };
                let (m, name) = rest.trim_start().split_once(' ')?;
                let mode = u32::from_str_radix(m, 8).ok()?;
                let name = name.trim();
                if name.is_empty() { return None; }
                hdr = Some((kind, name.to_string(), mode & 0o777, ln));
            } else if body < 20 && hdr.as_ref().unwrap().0 != TextKind::B64 {
                if s.bytes().skip(1).any(|c| c.is_ascii_lowercase()) { lower_seen = true; }
                body += 1;
            }
        }
        if let Some((mut kind, name, mode, l)) = hdr {
            if kind == TextKind::Uu && (lower_seen || ext == "xxe") { kind = TextKind::Xx; }
            return Some(TextHdr { kind, name, mode, skip_lines: l });
        }
        // header-less base64 only by explicit extension, and only when the head looks like base64
        if matches!(ext.as_str(), "b64" | "base64") {
            let mut h = [0u8; 4096];
            let n = File::open(path).ok()?.read(&mut h).ok()?;
            if n > 0 && h[..n].iter().all(|c| b64_val(*c).is_some() || matches!(c, b'=' | b'\r' | b'\n')) {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("decoded").to_string();
                return Some(TextHdr { kind: TextKind::B64, name: stem, mode: 0o644, skip_lines: 0 });
            }
        }
        None
    }

    fn bad(msg: &str) -> Error { Error::new(ErrorKind::BrokenArchive, msg.to_string()) }

    fn decode_text(path: &Path, h: &TextHdr, out: &mut dyn Write, cancel: &CancelToken) -> Result<TextInfo> {
        let mut rd = BufReader::with_capacity(CHUNK, File::open(path)?);
        let mut line = Vec::new();
        for _ in 0..h.skip_lines { line.clear(); rd.read_until(b'\n', &mut line)?; }
        let mut ended = false;
        let mut quad: Vec<u8> = Vec::with_capacity(4);
        let mut obuf: Vec<u8> = Vec::with_capacity(CHUNK + 64);
        let mut lines = 0u64;
        loop {
            line.clear();
            if rd.read_until(b'\n', &mut line)? == 0 { break; }
            lines += 1;
            if lines % 1024 == 0 { cancel.check()?; }
            while matches!(line.last(), Some(b'\n' | b'\r')) { line.pop(); }
            match h.kind {
                TextKind::B64 => {
                    if line.starts_with(b"====") { ended = true; break; }
                    for &c in &line {
                        if c == b'=' { quad.push(255); }
                        else if let Some(v) = b64_val(c) { quad.push(v); }
                        else if c == b' ' || c == b'\t' { continue } else { return Err(bad("invalid base64 character")); }
                        if quad.len() == 4 {
                            let pad = quad.iter().filter(|v| **v == 255).count();
                            let q: Vec<u32> = quad.iter().map(|v| if *v == 255 { 0 } else { *v as u32 }).collect();
                            let n = (q[0] << 18) | (q[1] << 12) | (q[2] << 6) | q[3];
                            let b = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
                            obuf.extend_from_slice(&b[..3 - pad.min(2)]);
                            quad.clear();
                        }
                    }
                }
                TextKind::Uu | TextKind::Xx => {
                    if line == b"end" || line.starts_with(b"end ") { ended = true; break; }
                    if line.is_empty() { continue; }
                    let val = |c: u8| if h.kind == TextKind::Uu { uu_val(c) } else { xx_val(c) };
                    let len = val(line[0]).ok_or_else(|| bad("invalid length character"))? as usize;
                    if len == 0 { continue; }
                    let mut produced = 0usize;
                    let mut i = 1usize;
                    while produced < len {
                        let mut v = [0u8; 4];
                        for k in 0..4 {
                            v[k] = match line.get(i + k) { Some(c) => val(*c).ok_or_else(|| bad("invalid uuencode character"))?, None => 0 };
                        }
                        i += 4;
                        let b = [(v[0] << 2) | (v[1] >> 4), (v[1] << 4) | (v[2] >> 2), (v[2] << 6) | v[3]];
                        let take = (len - produced).min(3);
                        obuf.extend_from_slice(&b[..take]);
                        produced += take;
                    }
                }
            }
            if obuf.len() >= CHUNK { out.write_all(&obuf)?; obuf.clear(); }
        }
        out.write_all(&obuf)?;
        if h.kind != TextKind::B64 && !ended { return Err(bad("truncated: missing `end` line")); }
        Ok(TextInfo { name: h.name.clone(), mode: h.mode })
    }

    fn extract_text(path: &Path, h: &TextHdr, o: &ExtractOptions, cancel: &CancelToken, progress: &Progress, mut report: ExtractReport) -> Result<ExtractReport> {
        let names = vec![(h.name.clone(), false)];
        let (_d, targets) = plan_targets(&names, path, o, &mut report)?;
        let Some(Some(t)) = targets.first() else { return Ok(report) };
        progress.set_totals(1, 0);
        if let Some(p) = t.path.parent() { std::fs::create_dir_all(p)?; }
        let ask = |p: &Path| o.on_conflict.as_ref().map(|f| f(p, None)).unwrap_or(ConflictChoice::Skip);
        let Some(p) = resolve_conflict(&t.path, o, &ask) else { report.skipped += 1; return Ok(report) };
        let mut n = 0u64;
        let r = {
            struct W<'a> { w: std::io::BufWriter<File>, n: &'a mut u64, p: &'a Progress }
            impl Write for W<'_> {
                fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.w.write_all(b)?; *self.n += b.len() as u64; self.p.add_written(b.len() as u64); Ok(b.len()) }
                fn flush(&mut self) -> std::io::Result<()> { self.w.flush() }
            }
            let mut w = W { w: std::io::BufWriter::with_capacity(CHUNK, File::create(&p)?), n: &mut n, p: progress };
            let r = decode_text(path, h, &mut w, cancel);
            let _ = w.flush();
            r
        };
        match r {
            Ok(info) => { set_mode(&p, info.mode); report.extracted += 1; report.bytes += n; progress.file_done(); Ok(report) }
            Err(e) => { let _ = std::fs::remove_file(&p); Err(e) }
        }
    }

    fn set_mode(p: &Path, mode: u32) {
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; let m = (mode & 0o777) | 0o600; let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)); }
        #[cfg(not(unix))]
        { let _ = (p, mode); }
    }
}
