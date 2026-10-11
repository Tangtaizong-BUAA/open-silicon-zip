//! Input enumeration: turns the user's selected paths into archive entries.
//!
//! Each input file becomes one entry named by its file name. Each input directory
//! becomes entries rooted at its basename (`proj/`, `proj/a.txt`, ...), empty
//! directories included. With exactly one directory input and `wrap_single_root`
//! off, the directory's contents are stored without the root folder prefix.

use crate::cancel::CancelToken;
use crate::error::{Error, ErrorKind, Result};
use crate::pathsafe::{ExcludeFilter, is_apple_junk};
use std::collections::HashSet;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

/// One archive entry to be written.
#[derive(Debug, Clone)]
pub struct EntryDesc {
    /// Source path on disk.
    pub abs: PathBuf,
    /// Archive-relative name, `/` separators, directories end with `/`.
    pub name: String,
    pub kind: EntryKind,
    /// Byte size of the content (file length, or link target length for symlinks).
    pub size: u64,
    pub mtime: Option<SystemTime>,
    /// Unix permission bits (`0o644` / `0o755` defaults on non-unix hosts).
    pub mode: u32,
    /// Link target, for `EntryKind::Symlink` only.
    pub link_target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkPolicy {
    /// Record the link itself.
    Store,
    /// Follow the link and store what it points to. Broken links and loops are skipped.
    Dereference,
}

impl Default for EnumOptions {
    fn default() -> Self {
        EnumOptions { symlinks: SymlinkPolicy::Store, exclude: None, include_apple_junk: false, wrap_single_root: true, deterministic: true }
    }
}

pub struct EnumOptions {
    pub symlinks: SymlinkPolicy,
    /// Entries matching this filter are left out (directories prune their subtree).
    pub exclude: Option<ExcludeFilter>,
    /// Keep `.DS_Store`, `__MACOSX` and similar macOS metadata.
    pub include_apple_junk: bool,
    /// With one directory input: `true` keeps the `proj/` root folder, `false` stores only its contents.
    pub wrap_single_root: bool,
    /// Sort the result by name (directories sort before their children).
    pub deterministic: bool,
}

/// Enumerates the archive entries for `inputs`.
///
/// Duplicate names are dropped (first occurrence wins). Returns `E_CANCELLED` if
/// `cancel` is set while walking. Non-UTF-8 names are rejected with `InvalidInput`.
pub fn enumerate(inputs: &[PathBuf], opts: &EnumOptions, cancel: &CancelToken) -> Result<Vec<EntryDesc>> {
    let deref = opts.symlinks == SymlinkPolicy::Dereference;
    let contents_only = !opts.wrap_single_root
        && inputs.len() == 1
        && match resolve(&inputs[0], deref)? {
            Some(md) => md.is_dir(),
            None => false,
        };

    let mut out: Vec<EntryDesc> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut push = |desc: EntryDesc, out: &mut Vec<EntryDesc>| {
        if seen.insert(desc.name.clone()) {
            out.push(desc);
        }
    };

    for input in inputs {
        cancel.check()?;
        let md = match resolve(input, deref)? {
            Some(md) => md,
            None => continue, // broken symlink under Dereference
        };
        let base = base_name(input)?;

        if !md.is_dir() {
            let desc = describe(input, base, &lstat_or(input, deref)?)?;
            if keep_name(&desc.name, false, opts) {
                push(desc, &mut out);
            }
            continue;
        }

        let prefix = if contents_only { String::new() } else { format!("{base}/") };
        if !contents_only && keep_name(&prefix, true, opts) {
            let desc = describe(input, prefix.clone(), &md)?;
            push(desc, &mut out);
        }

        let mut walker = WalkDir::new(input).follow_links(deref);
        if opts.deterministic {
            walker = walker.sort_by_file_name();
        }
        let entries = walker.into_iter().filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            match rel_name(e.path(), input, &prefix) {
                Ok(name) => keep_name(&name, e.file_type().is_dir(), opts),
                Err(_) => true, // reported when the entry is emitted below
            }
        });

        for item in entries {
            cancel.check()?;
            let e = match item {
                Ok(e) => e,
                Err(err) => {
                    if err.loop_ancestor().is_some() {
                        continue;
                    }
                    if err.io_error().map(|io| io.kind()) == Some(std::io::ErrorKind::NotFound) {
                        continue; // dangling link under Dereference
                    }
                    let io = err.into_io_error();
                    return Err(io.map(Error::from).unwrap_or_else(|| Error::new(ErrorKind::Io, "directory walk failed")));
                }
            };
            if e.depth() == 0 {
                continue; // root already handled above
            }
            let name = rel_name(e.path(), input, &prefix)?;
            let md = match e.metadata() {
                Ok(m) => m,
                Err(err) => {
                    if err.io_error().map(|io| io.kind()) == Some(std::io::ErrorKind::NotFound) {
                        continue;
                    }
                    return Err(err.into_io_error().map(Error::from).unwrap_or_else(|| Error::new(ErrorKind::Io, "metadata failed")));
                }
            };
            let name = if md.is_dir() { format!("{name}/") } else { name };
            let desc = describe(e.path(), name, &md)?;
            push(desc, &mut out);
        }
    }

    if opts.deterministic {
        out.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Ok(out)
}

/// Metadata of an input path. `None` for a symlink that cannot be followed under `Dereference`.
fn resolve(path: &Path, deref: bool) -> Result<Option<Metadata>> {
    let lmd = std::fs::symlink_metadata(path)?;
    if lmd.file_type().is_symlink() && deref {
        return Ok(std::fs::metadata(path).ok());
    }
    Ok(Some(lmd))
}

/// Link metadata for symlinks, follows otherwise.
fn lstat_or(path: &Path, deref: bool) -> Result<Metadata> {
    if deref {
        Ok(std::fs::metadata(path)?)
    } else {
        Ok(std::fs::symlink_metadata(path)?)
    }
}

fn base_name(path: &Path) -> Result<String> {
    // `.` and `..` have no file name, so resolve them first.
    let resolved;
    let name = match path.file_name() {
        Some(n) => n,
        None => {
            resolved = std::fs::canonicalize(path)?;
            resolved
                .file_name()
                .ok_or_else(|| Error::new(ErrorKind::InvalidInput, format!("cannot name input: {}", path.display())))?
        }
    };
    name.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, format!("name is not valid UTF-8: {}", path.display())))
}

/// Archive name for `path` under `root`, with `prefix` put in front.
fn rel_name(path: &Path, root: &Path, prefix: &str) -> Result<String> {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut parts = Vec::new();
    for comp in rel.components() {
        let s = comp
            .as_os_str()
            .to_str()
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, format!("name is not valid UTF-8: {}", path.display())))?;
        parts.push(s);
    }
    Ok(format!("{prefix}{}", parts.join("/")))
}

/// Applies the junk and exclude rules to an archive name (directories end with `/`).
fn keep_name(name: &str, is_dir: bool, opts: &EnumOptions) -> bool {
    let trimmed = name.trim_end_matches('/');
    if trimmed.is_empty() {
        return true;
    }
    if !opts.include_apple_junk && trimmed.split('/').any(|c| !c.is_empty() && is_apple_junk(c)) {
        return false;
    }
    if let Some(ex) = &opts.exclude {
        if ex.matches(trimmed, is_dir) {
            return false;
        }
    }
    true
}

fn describe(path: &Path, name: String, md: &Metadata) -> Result<EntryDesc> {
    let (kind, size, link_target) = if md.file_type().is_symlink() {
        let target = std::fs::read_link(path)?;
        let target = target
            .to_str()
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, format!("link target is not UTF-8: {}", path.display())))?
            .to_owned();
        (EntryKind::Symlink, target.len() as u64, Some(target))
    } else if md.is_dir() {
        (EntryKind::Dir, 0, None)
    } else {
        (EntryKind::File, md.len(), None)
    };
    Ok(EntryDesc {
        abs: path.to_path_buf(),
        name,
        kind,
        size,
        mtime: md.modified().ok(),
        mode: mode_of(md, kind),
        link_target,
    })
}

#[cfg(unix)]
fn mode_of(md: &Metadata, _kind: EntryKind) -> u32 {
    use std::os::unix::fs::MetadataExt;
    md.mode() & 0o777
}

#[cfg(not(unix))]
fn mode_of(_md: &Metadata, kind: EntryKind) -> u32 {
    if kind == EntryKind::Dir { 0o755 } else { 0o644 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn opts() -> EnumOptions {
        EnumOptions {
            symlinks: SymlinkPolicy::Store,
            exclude: None,
            include_apple_junk: false,
            wrap_single_root: true,
            deterministic: true,
        }
    }

    fn names(v: &[EntryDesc]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn tree() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("proj");
        write(&root.join("a.txt"), "a");
        write(&root.join("sub/b.txt"), "bb");
        fs::create_dir_all(root.join("empty")).unwrap();
        t
    }

    #[test]
    fn directory_structure_with_root_and_empty_dirs() {
        let t = tree();
        let got = enumerate(&[t.path().join("proj")], &opts(), &CancelToken::new()).unwrap();
        assert_eq!(names(&got), vec!["proj/", "proj/a.txt", "proj/empty/", "proj/sub/", "proj/sub/b.txt"]);
        let dir = got.iter().find(|e| e.name == "proj/empty/").unwrap();
        assert_eq!(dir.kind, EntryKind::Dir);
        assert_eq!(dir.size, 0);
        let file = got.iter().find(|e| e.name == "proj/sub/b.txt").unwrap();
        assert_eq!(file.kind, EntryKind::File);
        assert_eq!(file.size, 2);
        assert_eq!(file.abs, t.path().join("proj/sub/b.txt"));
        assert!(file.link_target.is_none());
    }

    #[test]
    fn single_file_uses_file_name() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("note.md");
        write(&f, "hi");
        let got = enumerate(&[f], &opts(), &CancelToken::new()).unwrap();
        assert_eq!(names(&got), vec!["note.md"]);
        assert_eq!(got[0].size, 2);
    }

    #[test]
    fn no_wrap_stores_contents_only() {
        let t = tree();
        let o = EnumOptions { wrap_single_root: false, ..opts() };
        let got = enumerate(&[t.path().join("proj")], &o, &CancelToken::new()).unwrap();
        assert_eq!(names(&got), vec!["a.txt", "empty/", "sub/", "sub/b.txt"]);
    }

    #[test]
    fn apple_junk_skipped_unless_included() {
        let t = tree();
        let root = t.path().join("proj");
        write(&root.join(".DS_Store"), "x");
        write(&root.join("__MACOSX/._a.txt"), "x");
        let got = enumerate(&[root.clone()], &opts(), &CancelToken::new()).unwrap();
        let n = names(&got);
        assert!(!n.iter().any(|x| x.contains("DS_Store") || x.contains("__MACOSX")), "{n:?}");
        assert!(n.contains(&"proj/a.txt"));

        let o = EnumOptions { include_apple_junk: true, ..opts() };
        let got = enumerate(&[root], &o, &CancelToken::new()).unwrap();
        let n = names(&got);
        assert!(n.iter().any(|x| x.ends_with(".DS_Store")));
        assert!(n.iter().any(|x| x.contains("__MACOSX")));
    }

    #[test]
    fn exclude_pattern_removes_matches() {
        let t = tree();
        let root = t.path().join("proj");
        write(&root.join("junk.tmp"), "x");
        write(&root.join("sub/more.tmp"), "x");
        let o = EnumOptions { exclude: Some(ExcludeFilter::new(&["*.tmp"])), ..opts() };
        let got = enumerate(&[root], &o, &CancelToken::new()).unwrap();
        let n = names(&got);
        assert!(!n.iter().any(|x| x.ends_with(".tmp")), "{n:?}");
        assert!(n.contains(&"proj/a.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_store_vs_dereference() {
        use std::os::unix::fs::symlink;
        let t = tree();
        let root = t.path().join("proj");
        symlink("a.txt", root.join("ln")).unwrap();
        symlink("missing", root.join("dangling")).unwrap();

        let got = enumerate(&[root.clone()], &opts(), &CancelToken::new()).unwrap();
        let ln = got.iter().find(|e| e.name == "proj/ln").unwrap();
        assert_eq!(ln.kind, EntryKind::Symlink);
        assert_eq!(ln.link_target.as_deref(), Some("a.txt"));
        assert_eq!(ln.size, 5);
        assert!(got.iter().any(|e| e.name == "proj/dangling" && e.kind == EntryKind::Symlink));

        let o = EnumOptions { symlinks: SymlinkPolicy::Dereference, ..opts() };
        let got = enumerate(&[root], &o, &CancelToken::new()).unwrap();
        let ln = got.iter().find(|e| e.name == "proj/ln").unwrap();
        assert_eq!(ln.kind, EntryKind::File);
        assert_eq!(ln.size, 1);
        assert!(ln.link_target.is_none());
        assert!(!got.iter().any(|e| e.name == "proj/dangling"));
    }

    #[cfg(unix)]
    #[test]
    fn dereference_terminates_on_directory_loop() {
        use std::os::unix::fs::symlink;
        let t = tree();
        let root = t.path().join("proj");
        symlink(&root, root.join("sub/up")).unwrap();
        let o = EnumOptions { symlinks: SymlinkPolicy::Dereference, ..opts() };
        let got = enumerate(&[root], &o, &CancelToken::new()).unwrap();
        assert!(got.iter().any(|e| e.name == "proj/sub/b.txt"));
        assert!(got.len() < 100);
    }

    #[test]
    fn duplicate_names_are_dropped() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("x/same.txt"), "1");
        write(&t.path().join("y/same.txt"), "2");
        let got = enumerate(
            &[t.path().join("x/same.txt"), t.path().join("y/same.txt")],
            &opts(),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(names(&got), vec!["same.txt"]);
        assert_eq!(got[0].size, 1);
    }

    #[test]
    fn deterministic_order_is_sorted_and_stable() {
        let t = tree();
        let root = t.path().join("proj");
        write(&root.join("z/deep/f.txt"), "x");
        write(&root.join("b.txt"), "x");
        let first = enumerate(&[root.clone()], &opts(), &CancelToken::new()).unwrap();
        let second = enumerate(&[root], &opts(), &CancelToken::new()).unwrap();
        assert_eq!(names(&first), names(&second));
        let n = names(&first);
        let mut sorted = n.clone();
        sorted.sort();
        assert_eq!(n, sorted);
        let dir_pos = n.iter().position(|x| *x == "proj/z/").unwrap();
        let child_pos = n.iter().position(|x| *x == "proj/z/deep/").unwrap();
        assert!(dir_pos < child_pos);
    }

    #[test]
    fn cancelled_token_stops_enumeration() {
        let t = tree();
        let token = CancelToken::new();
        token.cancel();
        let err = enumerate(&[t.path().join("proj")], &opts(), &token).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Cancelled);
    }

    #[test]
    fn missing_input_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        assert!(enumerate(&[t.path().join("nope")], &opts(), &CancelToken::new()).is_err());
    }
}
