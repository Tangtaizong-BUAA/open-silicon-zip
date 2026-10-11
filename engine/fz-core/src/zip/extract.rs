//! Parallel ZIP extraction with Smart Extract, zip-slip protection, conflict handling.
use super::format::*;
use super::reader::{EncKind, ZipArchive, ZipEntry};
use crate::cancel::{CancelToken, Progress};
use crate::error::{Error, ErrorKind, Result};
use crate::pathsafe::{self, is_apple_junk, safe_join};
use crate::smart;
use rayon::prelude::*;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Overwrite { Overwrite, Skip, Rename, Ask }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConflictChoice { Skip, Overwrite, Rename }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Normalization { Nfc, Nfd, None }

pub type ConflictFn = Arc<dyn Fn(&Path, Option<&ZipEntry>) -> ConflictChoice + Send + Sync>;

#[derive(Clone)]
pub struct ExtractOptions {
    pub dest: PathBuf,
    pub password: Option<String>,
    pub smart: bool,
    pub wrap_single_files: bool,
    pub filter_apple_junk: bool,
    /// Entry indices; None = all.
    pub selected: Option<Vec<usize>>,
    pub overwrite: Overwrite,
    pub on_conflict: Option<ConflictFn>,
    pub normalization: Normalization,
    pub threads: usize,
    pub delete_partial_on_cancel: bool,
    pub allow_external_symlinks: bool,
}

impl ExtractOptions {
    pub fn new(dest: impl Into<PathBuf>) -> Self {
        ExtractOptions {
            dest: dest.into(), password: None, smart: true, wrap_single_files: false, filter_apple_junk: true, selected: None,
            overwrite: Overwrite::Rename, on_conflict: None, normalization: Normalization::Nfc, threads: 0,
            delete_partial_on_cancel: true, allow_external_symlinks: false,
        }
    }
}

#[derive(Debug, Default)]
pub struct ExtractReport {
    pub extracted: u64,
    pub skipped: u64,
    pub bytes: u64,
    pub unsafe_refused: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub root: PathBuf,
}

fn normalize(s: &str, n: Normalization) -> String {
    use unicode_normalization::UnicodeNormalization;
    match n { Normalization::Nfc => s.nfc().collect(), Normalization::Nfd => s.nfd().collect(), Normalization::None => s.to_string() }
}

/// Archive-relative target (after smart stripping) or None when the entry is the stripped root itself.
fn rel_target(name: &str, strip: &Option<String>) -> Option<String> {
    let normalized = pathsafe::normalize_read_name(name);
    let mut n = normalized.as_str(); // absolute names are kept so safe_join refuses them
    if let Some(s) = strip {
        n = match n.strip_prefix(s.as_str()) {
            Some(r) if r.is_empty() || r.starts_with('/') => r.trim_start_matches('/'),
            _ => n,
        };
    }
    if n.is_empty() { None } else { Some(n.to_string()) }
}

pub fn extract(arc: &ZipArchive, o: &ExtractOptions, cancel: &CancelToken, progress: &Progress) -> Result<ExtractReport> {
    let mut report = ExtractReport::default();
    let sel: Vec<&ZipEntry> = arc.entries.iter()
        .filter(|e| o.selected.as_ref().map_or(true, |s| s.contains(&e.index)))
        .filter(|e| !(o.filter_apple_junk && is_apple_junk(&e.name)))
        .collect();

    let mut dest = o.dest.clone();
    let mut strip = None;
    if o.smart && o.selected.is_none() {
        let stem = arc.vols.paths.last().and_then(|p| p.file_name()).and_then(|n| n.to_str()).map(smart::archive_stem).unwrap_or_else(|| "archive".into());
        let pairs: Vec<(String, bool)> = sel.iter().map(|e| (e.name.clone(), e.is_dir)).collect();
        let plan = smart::plan(&pairs, &stem, o.wrap_single_files);
        strip = plan.strip;
        if let Some(w) = plan.wrap { dest = dest.join(normalize(&w, o.normalization)); }
    }
    pathsafe::ensure_no_symlink(&o.dest, &dest)?;
    fs::create_dir_all(&dest)?;
    report.root = dest.clone();
    progress.set_totals(sel.len() as u64, sel.iter().map(|e| e.size).sum());
    progress.set_phase("extracting");

    // resolve targets (zip-slip checks) up front
    struct Job<'a> { e: &'a ZipEntry, path: PathBuf, rel: String }
    let mut jobs: Vec<Job> = Vec::new();
    for e in &sel {
        let Some(rel) = rel_target(&e.name, &strip) else { continue };
        let rel = rel.split('/').map(|c| normalize(c, o.normalization)).collect::<Vec<_>>().join("/");
        match safe_join(&dest, &rel) {
            Ok(p) => {
                if pathsafe::ensure_no_symlink(&dest, &p).is_ok() { jobs.push(Job { e, path: p, rel }); }
                else { report.unsafe_refused.push(e.name.clone()); }
            },
            Err(_) => report.unsafe_refused.push(e.name.clone()),
        }
    }

    // Refuse colliding normalized/case-folded names before parallel workers race.
    let mut names = std::collections::HashSet::new();
    jobs.retain(|j| {
        if names.insert(j.path.to_string_lossy().to_lowercase()) { true }
        else { report.unsafe_refused.push(j.e.name.clone()); false }
    });

    // directories first (sequential, cheap)
    for j in jobs.iter().filter(|j| j.e.is_dir) { fs::create_dir_all(&j.path)?; }
    // parent dirs for files
    {
        let mut seen = std::collections::HashSet::new();
        for j in jobs.iter().filter(|j| !j.e.is_dir) {
            if let Some(p) = j.path.parent() { if seen.insert(p.to_path_buf()) { fs::create_dir_all(p)?; } }
        }
    }

    let created: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    let stats = Mutex::new((0u64, 0u64, 0u64, Vec::<(String, String)>::new()));
    let pool = crate::sys::build_pool(o.threads);
    let files: Vec<&Job> = jobs.iter().filter(|j| !j.e.is_dir && !j.e.is_symlink).collect();

    let res: Result<()> = pool.install(|| {
        files.par_iter().try_for_each(|j| -> Result<()> {
            cancel.check()?;
            progress.set_current(&j.rel);
            let mut target = j.path.clone();
            let mut replace = false;
            if fs::symlink_metadata(&target).is_ok() {
                let choice = match o.overwrite {
                    Overwrite::Overwrite => ConflictChoice::Overwrite,
                    Overwrite::Skip => ConflictChoice::Skip,
                    Overwrite::Rename => ConflictChoice::Rename,
                    Overwrite::Ask => o.on_conflict.as_ref().map(|f| f(&target, Some(j.e))).unwrap_or(ConflictChoice::Skip),
                };
                match choice {
                    ConflictChoice::Skip => { stats.lock().unwrap().1 += 1; progress.file_done(); return Ok(()); }
                    ConflictChoice::Rename => { target = pathsafe::unique_name(&target); }
                    ConflictChoice::Overwrite => { replace = true; }
                }
            }
            pathsafe::ensure_no_symlink(&dest, &target)?;
            let replaces_existing = fs::symlink_metadata(&target).is_ok();
            let temporary = tempfile::NamedTempFile::new_in(target.parent().unwrap_or(&dest))?;
            let f = temporary.as_file();
            if j.e.size > (1 << 20) { let _ = f.set_len(j.e.size); }
            let mut w = BufWriter::with_capacity(1 << 20, f);
            let r = arc.read_entry(j.e, o.password.as_deref(), &mut CancelWriter { w: &mut w, c: cancel }).and_then(|n| { w.flush()?; Ok(n) });
            match r {
                Ok(n) => {
                    drop(w);
                    if replace {
                        temporary.persist(&target).map_err(|e| Error::from(e.error))?;
                    } else {
                        temporary.persist_noclobber(&target).map_err(|e| Error::from(e.error))?;
                    }
                    if !replaces_existing { created.lock().unwrap().push(target.clone()); }
                    let _ = set_meta(&target, j.e, false);
                    progress.add_read(j.e.csize); progress.add_written(n); progress.file_done();
                    let mut s = stats.lock().unwrap(); s.0 += 1; s.2 += n;
                    Ok(())
                }
                Err(e) if e.kind == ErrorKind::Cancelled => Err(e),
                Err(e) if matches!(e.kind, ErrorKind::PasswordRequired | ErrorKind::WrongPassword) => { drop(w); Err(e) }
                Err(e) => {
                    drop(w);
                    stats.lock().unwrap().3.push((j.e.name.clone(), e.to_string()));
                    progress.file_done();
                    Ok(())
                }
            }
        })
    });
    if let Err(e) = res {
        // Incomplete temporary files are removed by RAII. Never delete an overwritten original.
        if e.kind == ErrorKind::Cancelled && o.delete_partial_on_cancel {
            for p in created.lock().unwrap().iter() { let _ = fs::remove_file(p); }
        }
        return Err(e);
    }

    // symlinks
    for j in jobs.iter().filter(|j| j.e.is_symlink) {
        if j.e.size > 65536 { report.unsafe_refused.push(j.e.name.clone()); continue; }
        pathsafe::ensure_no_symlink(&dest, &j.path)?;
        let mut buf = Vec::new();
        if arc.read_entry(j.e, o.password.as_deref(), &mut buf).is_err() { continue; }
        let target = String::from_utf8_lossy(&buf).into_owned();
        let parent_rel = Path::new(&j.rel).parent().unwrap_or(Path::new("")).to_path_buf();
        if !o.allow_external_symlinks && !pathsafe::symlink_target_safe(&dest, &parent_rel, &target) {
            report.unsafe_refused.push(format!("{} -> {}", j.e.name, target));
            continue;
        }
        let _ = fs::remove_file(&j.path);
        #[cfg(unix)]
        if std::os::unix::fs::symlink(&target, &j.path).is_ok() { stats.lock().unwrap().0 += 1; }
    }

    // metadata pass: permissions + timestamps, directories deepest-first
    let mut dirs: Vec<&Job> = jobs.iter().filter(|j| j.e.is_dir).collect();
    dirs.sort_by_key(|j| std::cmp::Reverse(j.path.components().count()));
    for j in dirs { let _ = set_meta(&j.path, j.e, true); }

    let s = stats.into_inner().unwrap();
    report.extracted = s.0; report.skipped = s.1; report.bytes = s.2; report.failed = s.3;
    Ok(report)
}

fn set_meta(p: &Path, e: &ZipEntry, dir: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut m = e.mode & 0o777;
        if m == 0 { m = if dir { 0o755 } else { 0o644 }; }
        if dir { m |= 0o700; } else { m |= 0o400; }
        fs::set_permissions(p, fs::Permissions::from_mode(m))?;
    }
    let t = filetime::FileTime::from_unix_time(e.mtime, 0);
    filetime::set_file_mtime(p, t)
}

struct CancelWriter<'a, W: Write> { w: &'a mut W, c: &'a CancelToken }
impl<W: Write> Write for CancelWriter<'_, W> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.c.is_cancelled() { return Err(crate::cancel::io_cancelled()); }
        self.w.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.w.flush() }
}

/// Test archive (FR-701): decode + verify every entry in parallel. Returns per-entry results.
pub fn test_archive(arc: &ZipArchive, password: Option<&str>, cancel: &CancelToken, progress: &Progress) -> Result<Vec<(String, Result<()>)>> {
    progress.set_phase("testing");
    progress.set_totals(arc.entries.len() as u64, arc.entries.iter().map(|e| e.size).sum());
    let pool = crate::sys::build_pool(0);
    let r: Vec<Option<(String, Result<()>)>> = pool.install(|| arc.entries.par_iter().map(|e| {
        if cancel.is_cancelled() { return None; }
        let r = arc.read_entry(e, password, &mut std::io::sink()).map(|n| { progress.add_read(n); });
        progress.file_done();
        Some((e.name.clone(), r))
    }).collect());
    cancel.check()?;
    Ok(r.into_iter().flatten().collect())
}

#[allow(dead_code)]
fn _unused(_: EncKind, _: u16) { let _ = M_STORE; }
