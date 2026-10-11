//! Shared extraction planning for non-ZIP formats: smart extract, junk filter, zip-slip, conflicts.
use crate::error::Result;
use crate::pathsafe::{self, is_apple_junk, safe_join};
use crate::smart;
use crate::zip::extract::{ConflictChoice, ExtractOptions, ExtractReport, Normalization, Overwrite};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Target {
    pub path: PathBuf,
    pub rel: String,
}

pub fn norm(s: &str, n: Normalization) -> String {
    use unicode_normalization::UnicodeNormalization;
    match n { Normalization::Nfc => s.nfc().collect(), Normalization::Nfd => s.nfd().collect(), Normalization::None => s.to_string() }
}

/// For each `(name, is_dir)` return its destination (None = filtered, stripped root or refused).
pub fn plan_targets(names: &[(String, bool)], archive_file: &Path, o: &ExtractOptions, report: &mut ExtractReport) -> Result<(PathBuf, Vec<Option<Target>>)> {
    let keep: Vec<bool> = names.iter().enumerate().map(|(i, (n, _))| {
        !(o.filter_apple_junk && is_apple_junk(n)) && o.selected.as_ref().map_or(true, |s| s.contains(&i))
    }).collect();
    let mut dest = o.dest.clone();
    let mut strip: Option<String> = None;
    if o.smart && o.selected.is_none() {
        let stem = archive_file.file_name().and_then(|n| n.to_str()).map(smart::archive_stem).unwrap_or_else(|| "archive".into());
        let pairs: Vec<(String, bool)> = names.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| p.clone()).collect();
        let plan = smart::plan(&pairs, &stem, o.wrap_single_files);
        strip = plan.strip;
        if let Some(w) = plan.wrap { dest = dest.join(norm(&w, o.normalization)); }
    }
    pathsafe::ensure_no_symlink(&o.dest, &dest)?;
    std::fs::create_dir_all(&dest)?;
    let mut out = Vec::with_capacity(names.len());
    let mut seen = std::collections::HashSet::new();
    for (i, (name, _)) in names.iter().enumerate() {
        if !keep[i] { out.push(None); continue; }
        let mut n = name.replace('\\', "/");
        while let Some(r) = n.strip_prefix("./") { n = r.to_string(); }
        let mut n = n.to_string(); // leading '/' kept: safe_join refuses absolute names
        if let Some(s) = &strip {
            if n == *s || n == format!("{s}/") { out.push(None); continue; }
            if let Some(r) = n.strip_prefix(&format!("{s}/")) { n = r.to_string(); }
        }
        if n.is_empty() { out.push(None); continue; }
        let rel = n.split('/').map(|c| norm(c, o.normalization)).collect::<Vec<_>>().join("/");
        match safe_join(&dest, &rel) {
            Ok(p) => {
                if pathsafe::ensure_no_symlink(&dest, &p).is_ok() && seen.insert(p.to_string_lossy().to_lowercase()) {
                    out.push(Some(Target { path: p, rel }));
                } else { report.unsafe_refused.push(name.clone()); out.push(None); }
            },
            Err(_) => { report.unsafe_refused.push(name.clone()); out.push(None); }
        }
    }
    report.root = dest.clone();
    Ok((dest, out))
}

/// Apply overwrite policy. Returns the path to write, or None to skip.
pub fn resolve_conflict(target: &Path, o: &ExtractOptions, ask: &dyn Fn(&Path) -> ConflictChoice) -> Option<PathBuf> {
    if std::fs::symlink_metadata(target).is_err() { return Some(target.to_path_buf()); }
    let choice = match o.overwrite {
        Overwrite::Overwrite => ConflictChoice::Overwrite,
        Overwrite::Skip => ConflictChoice::Skip,
        Overwrite::Rename => ConflictChoice::Rename,
        Overwrite::Ask => ask(target),
    };
    match choice {
        ConflictChoice::Skip => None,
        ConflictChoice::Rename => Some(pathsafe::unique_name(target)),
        ConflictChoice::Overwrite => Some(target.to_path_buf())
    }
}

pub fn apply_meta(p: &Path, mode: u32, mtime: i64, dir: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut m = mode & 0o777;
        if m == 0 { m = if dir { 0o755 } else { 0o644 }; }
        m |= if dir { 0o700 } else { 0o400 };
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(m));
    }
    let _ = filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(mtime, 0));
}
