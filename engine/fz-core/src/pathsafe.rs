//! Path sanitisation and safety checks for archive entry names.
//!
//! Everything here is lexical (no filesystem access), except `unique_name`,
//! which probes for existing paths to pick a free name.
use std::path::{Path, PathBuf};

use crate::error::{Error, ErrorKind, Result};

/// Refuse archive-created or pre-existing symlinks below the chosen destination.
/// The destination itself may be a user-selected alias; children may not redirect writes.
/// This does not protect against another process concurrently replacing directories.
pub fn ensure_no_symlink(dest: &Path, target: &Path) -> Result<()> {
    let relative = target.strip_prefix(dest).map_err(|_| Error::new(ErrorKind::UnsafePath, "target outside destination"))?;
    let mut current = dest.to_path_buf();
    // A smart-extraction wrapper can itself be a hostile pre-existing symlink.
    if std::fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(Error::new(ErrorKind::UnsafePath, "symlink extraction root"));
    }
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(Error::new(ErrorKind::UnsafePath, "invalid target component"));
        }
        current.push(component);
        if std::fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(Error::new(ErrorKind::UnsafePath, format!("symlink in extraction path: {}", current.display())));
        }
    }
    Ok(())
}

/// Maximum length of an entry name in bytes.
const MAX_ENTRY_NAME_BYTES: usize = 65535;

/// True if the string starts with a Windows drive designator such as `C:`.
fn has_drive_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// Normalises a name for writing into an archive.
///
/// Converts `\` to `/`, strips leading `./`, `/` and drive letters, collapses
/// repeated separators, drops `.` components, replaces `:` with `_` inside
/// components, and keeps a trailing `/` for directories. Rejects empty results,
/// NUL bytes, any `..` component, and names longer than 65535 bytes.
pub fn sanitize_entry_name(name: &str) -> Result<String> {
    if name.contains('\0') {
        return Err(Error::new(ErrorKind::InvalidInput, "entry name contains NUL byte"));
    }
    let converted = name.replace('\\', "/");
    let trailing_slash = converted.ends_with('/');

    let mut rest = converted.as_str();
    loop {
        if let Some(r) = rest.strip_prefix("./") {
            rest = r;
        } else if let Some(r) = rest.strip_prefix('/') {
            rest = r;
        } else if has_drive_prefix(rest) {
            rest = &rest[2..];
        } else {
            break;
        }
    }

    let mut parts: Vec<String> = Vec::new();
    for comp in rest.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("entry name contains '..': {name:?}"),
                ));
            }
            c => parts.push(c.replace(':', "_")),
        }
    }
    if parts.is_empty() {
        return Err(Error::new(ErrorKind::InvalidInput, format!("entry name is empty: {name:?}")));
    }

    let mut out = parts.join("/");
    if trailing_slash {
        out.push('/');
    }
    if out.len() > MAX_ENTRY_NAME_BYTES {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("entry name exceeds {MAX_ENTRY_NAME_BYTES} bytes"),
        ));
    }
    Ok(out)
}

/// Normalises a name read from an archive made on Windows: `\` becomes `/`
/// and leading `./` prefixes are removed. Does not otherwise validate.
pub fn normalize_read_name(name: &str) -> String {
    let converted = name.replace('\\', "/");
    let mut rest = converted.as_str();
    while let Some(r) = rest.strip_prefix("./") {
        rest = r;
    }
    rest.to_string()
}

/// Joins an archive entry name onto `dest` for extraction (zip-slip protection, FR-216).
///
/// Rejects (`UnsafePath`) absolute names, drive-letter names, `..` components and
/// NUL bytes. Backslashes are treated as separators so Windows-made archives
/// cannot smuggle traversal through them. Empty names are `InvalidInput`.
/// Windows reserved device names are not rejected here; see `sanitize_for_windows`.
pub fn safe_join(dest: &Path, entry: &str) -> Result<PathBuf> {
    let unsafe_err = |why: &str| Error::new(ErrorKind::UnsafePath, format!("{why}: {entry:?}"));

    if entry.contains('\0') {
        return Err(unsafe_err("NUL byte in entry name"));
    }
    let converted = entry.replace('\\', "/");
    if converted.starts_with('/') {
        return Err(unsafe_err("absolute entry path"));
    }
    if has_drive_prefix(&converted) {
        return Err(unsafe_err("drive-letter entry path"));
    }

    let mut out = dest.to_path_buf();
    let mut any = false;
    for comp in converted.split('/') {
        match comp {
            "" | "." => {}
            ".." => return Err(unsafe_err("parent directory component")),
            c if has_drive_prefix(c) => return Err(unsafe_err("drive-letter component")),
            c => {
                out.push(c);
                any = true;
            }
        }
    }
    if !any {
        return Err(Error::new(ErrorKind::InvalidInput, format!("empty entry name: {entry:?}")));
    }
    if !out.starts_with(dest) {
        return Err(unsafe_err("entry escapes destination"));
    }
    Ok(out)
}

/// True if a single path component is a reserved Windows device name
/// (CON, PRN, AUX, NUL, COM1-9, LPT1-9), with or without an extension.
pub fn is_windows_reserved(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        s if s.len() == 4 && (s.starts_with("COM") || s.starts_with("LPT")) => {
            matches!(s.as_bytes()[3], b'1'..=b'9')
        }
        _ => false,
    }
}

/// Makes a single component safe to create on Windows by prefixing `_`
/// when it is a reserved device name. Other components are returned unchanged.
pub fn sanitize_for_windows(component: &str) -> String {
    if is_windows_reserved(component) {
        format!("_{component}")
    } else {
        component.to_string()
    }
}

/// Lexically checks that a symlink with `target` created in directory
/// `link_parent_rel` (relative to `dest`) resolves to a location inside `dest`.
///
/// Absolute targets, drive-letter targets, NUL bytes and any target or parent
/// path that climbs above `dest` return false. `dest` is accepted for API
/// symmetry; resolution is relative to it and never touches the filesystem.
pub fn symlink_target_safe(_dest: &Path, link_parent_rel: &Path, target: &str) -> bool {
    use std::path::Component;

    if target.is_empty() || target.contains('\0') {
        return false;
    }
    if target.starts_with('/') || target.starts_with('\\') || has_drive_prefix(target) {
        return false;
    }

    let mut stack: Vec<String> = Vec::new();
    for comp in link_parent_rel.components() {
        match comp {
            Component::Normal(s) => stack.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                if stack.pop().is_none() {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }

    for comp in target.replace('\\', "/").split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return false;
                }
            }
            c => stack.push(c.to_string()),
        }
    }
    true
}

/// True for macOS archive metadata and Windows/Finder noise:
/// `__MACOSX/` trees, `._*` AppleDouble files, `.DS_Store`, and `Thumbs.db`
/// (case-insensitive). `desktop.ini` is not treated as junk.
pub fn is_apple_junk(name: &str) -> bool {
    let converted = name.replace('\\', "/");
    let comps: Vec<&str> = converted.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    if comps.first() == Some(&"__MACOSX") {
        return true;
    }
    comps.iter().any(|c| c.starts_with("._") || *c == ".DS_Store" || c.eq_ignore_ascii_case("Thumbs.db"))
}

/// Returns `path` if nothing exists there, otherwise `stem (n).ext` for the
/// smallest n >= 1 that is free. Broken symlinks count as existing.
pub fn unique_name(path: &Path) -> PathBuf {
    if std::fs::symlink_metadata(path).is_err() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    let mut n: u64 = 1;
    loop {
        let file = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(file);
        if std::fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
        n += 1;
    }
}

/// One exclude pattern, pre-split into segments.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pattern {
    segments: Vec<String>,
    /// Pattern contained a `/`, so it matches from the root of the archive.
    anchored: bool,
    /// Pattern ended with `/`, so it only matches directories.
    dir_only: bool,
}

/// Glob-based exclude filter. Supports `*`, `?` and `**` (as a whole segment).
///
/// - A pattern without `/` matches any single path component.
/// - A pattern containing `/` matches from the archive root (a leading `/` is allowed).
/// - A trailing `/` restricts the pattern to directories; anything below a
///   matching directory is also excluded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExcludeFilter {
    patterns: Vec<Pattern>,
}

impl ExcludeFilter {
    /// Builds a filter from glob patterns. Empty patterns are ignored.
    pub fn new<I, S>(patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let patterns = patterns.into_iter().filter_map(|p| Pattern::parse(p.as_ref())).collect();
        ExcludeFilter { patterns }
    }

    /// True if the filter has no effective patterns.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// True if the relative path (or any ancestor directory of it) is excluded.
    /// `is_dir` describes the final component of `rel_path`.
    pub fn matches(&self, rel_path: &str, is_dir: bool) -> bool {
        let normalized = rel_path.replace('\\', "/");
        let comps: Vec<&str> = normalized.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
        if comps.is_empty() {
            return false;
        }
        self.patterns.iter().any(|p| p.matches(&comps, is_dir))
    }
}

impl Pattern {
    fn parse(raw: &str) -> Option<Pattern> {
        let normalized = raw.replace('\\', "/");
        let dir_only = normalized.ends_with('/');
        let trimmed = normalized.trim_end_matches('/');
        let anchored = trimmed.contains('/');
        let body = trimmed.strip_prefix("./").unwrap_or(trimmed).trim_start_matches('/');
        let segments: Vec<String> =
            body.split('/').filter(|s| !s.is_empty() && *s != ".").map(str::to_string).collect();
        if segments.is_empty() {
            return None;
        }
        // Without a '/', the pattern is a single segment matched against any component.
        debug_assert!(anchored || segments.len() == 1);
        Some(Pattern { segments, anchored, dir_only })
    }

    fn matches(&self, comps: &[&str], is_dir: bool) -> bool {
        let n = comps.len();
        // Component `i` is a directory if more components follow it, or if the caller says so.
        let is_dir_at = |i: usize| i + 1 < n || is_dir;
        let dir_ok = |i: usize| !self.dir_only || is_dir_at(i);

        if self.anchored {
            (1..=n).any(|k| dir_ok(k - 1) && match_segments(&self.segments, &comps[..k]))
        } else {
            let seg = &self.segments[0];
            comps.iter().enumerate().any(|(i, c)| dir_ok(i) && glob_match(seg, c))
        }
    }
}

/// Matches a sequence of pattern segments against a sequence of path components,
/// consuming all of both. `**` matches zero or more components.
fn match_segments(pat: &[String], comps: &[&str]) -> bool {
    match pat.split_first() {
        None => comps.is_empty(),
        Some((head, rest)) if head == "**" => {
            (0..=comps.len()).any(|skip| match_segments(rest, &comps[skip..]))
        }
        Some((head, rest)) => match comps.split_first() {
            Some((c, crest)) => glob_match(head, c) && match_segments(rest, crest),
            None => false,
        },
    }
}

/// Wildcard match of one segment: `*` matches any run of characters, `?` exactly one.
fn glob_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest() -> &'static Path {
        Path::new("/dest")
    }

    fn assert_unsafe<T: std::fmt::Debug>(r: Result<T>) {
        match r {
            Err(e) => assert_eq!(e.kind, ErrorKind::UnsafePath, "wrong kind: {e}"),
            Ok(v) => panic!("expected UnsafePath, got Ok({v:?})"),
        }
    }

    fn assert_invalid<T: std::fmt::Debug>(r: Result<T>) {
        match r {
            Err(e) => assert_eq!(e.kind, ErrorKind::InvalidInput, "wrong kind: {e}"),
            Ok(v) => panic!("expected InvalidInput, got Ok({v:?})"),
        }
    }

    // ---- sanitize_entry_name ----

    #[test]
    fn sanitize_plain_names_unchanged() {
        assert_eq!(sanitize_entry_name("a/b.txt").unwrap(), "a/b.txt");
        assert_eq!(sanitize_entry_name("file").unwrap(), "file");
        assert_eq!(sanitize_entry_name(".hidden").unwrap(), ".hidden");
    }

    #[test]
    fn sanitize_converts_backslashes() {
        assert_eq!(sanitize_entry_name("a\\b\\c.txt").unwrap(), "a/b/c.txt");
    }

    #[test]
    fn sanitize_strips_leading_dot_slash_and_root() {
        assert_eq!(sanitize_entry_name("./x").unwrap(), "x");
        assert_eq!(sanitize_entry_name("././x").unwrap(), "x");
        assert_eq!(sanitize_entry_name("/abs/x").unwrap(), "abs/x");
        assert_eq!(sanitize_entry_name("\\abs\\x").unwrap(), "abs/x");
    }

    #[test]
    fn sanitize_strips_drive_letters() {
        assert_eq!(sanitize_entry_name("C:\\x").unwrap(), "x");
        assert_eq!(sanitize_entry_name("c:/a/b").unwrap(), "a/b");
        assert_eq!(sanitize_entry_name("D:\\dir\\file.txt").unwrap(), "dir/file.txt");
    }

    #[test]
    fn sanitize_replaces_colon_in_components() {
        assert_eq!(sanitize_entry_name("ab:c/d:e").unwrap(), "ab_c/d_e");
        assert_eq!(sanitize_entry_name("a:b/c").unwrap(), "b/c", "leading X: is treated as a drive");
        assert_eq!(sanitize_entry_name("dir/x:y.txt").unwrap(), "dir/x_y.txt");
    }

    #[test]
    fn sanitize_collapses_separators_and_dots() {
        assert_eq!(sanitize_entry_name("a//b///c").unwrap(), "a/b/c");
        assert_eq!(sanitize_entry_name("a/./b").unwrap(), "a/b");
    }

    #[test]
    fn sanitize_keeps_trailing_slash_for_dirs() {
        assert_eq!(sanitize_entry_name("dir/").unwrap(), "dir/");
        assert_eq!(sanitize_entry_name("dir\\").unwrap(), "dir/");
        assert_eq!(sanitize_entry_name("a//b//").unwrap(), "a/b/");
    }

    #[test]
    fn sanitize_rejects_parent_components() {
        assert_invalid(sanitize_entry_name("../x"));
        assert_invalid(sanitize_entry_name("a/../../x"));
        assert_invalid(sanitize_entry_name("a\\..\\..\\b"));
        assert_invalid(sanitize_entry_name("/../x"));
        assert_invalid(sanitize_entry_name("C:..\\x"));
        assert_invalid(sanitize_entry_name("a/.."));
    }

    #[test]
    fn sanitize_rejects_nul() {
        assert_invalid(sanitize_entry_name("a\0b"));
    }

    #[test]
    fn sanitize_rejects_empty_results() {
        assert_invalid(sanitize_entry_name(""));
        assert_invalid(sanitize_entry_name("./"));
        assert_invalid(sanitize_entry_name("/"));
        assert_invalid(sanitize_entry_name("C:"));
        assert_invalid(sanitize_entry_name("."));
    }

    #[test]
    fn sanitize_length_limit() {
        let ok = "a".repeat(MAX_ENTRY_NAME_BYTES);
        assert_eq!(sanitize_entry_name(&ok).unwrap().len(), MAX_ENTRY_NAME_BYTES);
        let too_long = "a".repeat(MAX_ENTRY_NAME_BYTES + 1);
        assert_invalid(sanitize_entry_name(&too_long));
    }

    // ---- normalize_read_name ----

    #[test]
    fn normalize_read_converts_and_strips() {
        assert_eq!(normalize_read_name("dir\\file.txt"), "dir/file.txt");
        assert_eq!(normalize_read_name("./a"), "a");
        assert_eq!(normalize_read_name("././a\\b"), "a/b");
        assert_eq!(normalize_read_name(".hidden/x"), ".hidden/x");
        assert_eq!(normalize_read_name("plain"), "plain");
    }

    // ---- safe_join ----

    #[test]
    fn safe_join_normal_paths() {
        assert_eq!(safe_join(dest(), "a/b.txt").unwrap(), PathBuf::from("/dest/a/b.txt"));
        assert_eq!(safe_join(dest(), "file").unwrap(), PathBuf::from("/dest/file"));
        assert_eq!(safe_join(dest(), "a\\b.txt").unwrap(), PathBuf::from("/dest/a/b.txt"));
        assert_eq!(safe_join(dest(), "./a/./b").unwrap(), PathBuf::from("/dest/a/b"));
        assert_eq!(safe_join(dest(), "dir/").unwrap(), PathBuf::from("/dest/dir"));
    }

    #[test]
    fn safe_join_result_is_inside_dest() {
        for entry in ["a/b", "x/y/z", "./q", "w\\v"] {
            let out = safe_join(dest(), entry).unwrap();
            assert!(out.starts_with(dest()), "{out:?} escapes dest");
        }
    }

    #[test]
    fn safe_join_rejects_absolute() {
        assert_unsafe(safe_join(dest(), "/etc/passwd"));
        assert_unsafe(safe_join(dest(), "\\windows\\system32"));
    }

    #[test]
    fn safe_join_rejects_drive_letters() {
        assert_unsafe(safe_join(dest(), "C:\\x"));
        assert_unsafe(safe_join(dest(), "C:/x"));
        assert_unsafe(safe_join(dest(), "c:x"));
        assert_unsafe(safe_join(dest(), "a/C:foo"));
    }

    #[test]
    fn safe_join_rejects_parent_components() {
        assert_unsafe(safe_join(dest(), "../x"));
        assert_unsafe(safe_join(dest(), "a/../../x"));
        assert_unsafe(safe_join(dest(), "a\\..\\..\\b"));
        assert_unsafe(safe_join(dest(), "a/../b"));
        assert_unsafe(safe_join(dest(), ".."));
    }

    #[test]
    fn safe_join_rejects_nul() {
        assert_unsafe(safe_join(dest(), "a\0b"));
    }

    #[test]
    fn safe_join_empty_is_invalid_input() {
        assert_invalid(safe_join(dest(), ""));
        assert_invalid(safe_join(dest(), "./"));
        assert_invalid(safe_join(dest(), "."));
    }

    #[test]
    fn safe_join_does_not_reject_reserved_names() {
        assert_eq!(safe_join(dest(), "CON").unwrap(), PathBuf::from("/dest/CON"));
        assert_eq!(safe_join(dest(), "dir/nul.txt").unwrap(), PathBuf::from("/dest/dir/nul.txt"));
    }

    // ---- reserved names ----

    #[test]
    fn reserved_names_detected() {
        for name in ["CON", "con", "PRN", "aux", "NUL", "COM1", "com9", "LPT1", "lpt9", "nul.txt", "CON.tar.gz", "COM5.log", "CON "] {
            assert!(is_windows_reserved(name), "{name} should be reserved");
        }
    }

    #[test]
    fn reserved_names_not_matched_falsely() {
        for name in ["", "CONSOLE", "conx", "COM0", "COM10", "LPT0", "COMX", "xCON", ".CON", "readme", "nullable"] {
            assert!(!is_windows_reserved(name), "{name} should not be reserved");
        }
    }

    #[test]
    fn sanitize_for_windows_prefixes_reserved() {
        assert_eq!(sanitize_for_windows("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_for_windows("lpt1"), "_lpt1");
        assert_eq!(sanitize_for_windows("readme.txt"), "readme.txt");
    }

    // ---- symlink_target_safe ----

    #[test]
    fn symlink_relative_inside_is_safe() {
        let p = Path::new("a/b");
        assert!(symlink_target_safe(dest(), p, "x"));
        assert!(symlink_target_safe(dest(), p, "../c"));
        assert!(symlink_target_safe(dest(), p, "../../x"));
        assert!(symlink_target_safe(dest(), p, "../../a/b/../y"));
        assert!(symlink_target_safe(dest(), p, "."));
        assert!(symlink_target_safe(dest(), Path::new(""), "x/../y"));
    }

    #[test]
    fn symlink_escaping_is_unsafe() {
        let p = Path::new("a/b");
        assert!(!symlink_target_safe(dest(), p, "../../../x"));
        assert!(!symlink_target_safe(dest(), p, "../../.."));
        assert!(!symlink_target_safe(dest(), Path::new(""), "../x"));
        assert!(!symlink_target_safe(dest(), Path::new(""), "a/../../x"));
        assert!(!symlink_target_safe(dest(), Path::new("a"), "..\\..\\x"));
    }

    #[test]
    fn symlink_absolute_and_drive_targets_unsafe() {
        let p = Path::new("a");
        assert!(!symlink_target_safe(dest(), p, "/etc/passwd"));
        assert!(!symlink_target_safe(dest(), p, "\\etc\\passwd"));
        assert!(!symlink_target_safe(dest(), p, "C:\\x"));
        assert!(!symlink_target_safe(dest(), p, "c:/x"));
    }

    #[test]
    fn symlink_empty_nul_and_bad_parent_unsafe() {
        assert!(!symlink_target_safe(dest(), Path::new("a"), ""));
        assert!(!symlink_target_safe(dest(), Path::new("a"), "x\0y"));
        assert!(!symlink_target_safe(dest(), Path::new("../a"), "x"));
        assert!(!symlink_target_safe(dest(), Path::new("/abs"), "x"));
    }

    // ---- is_apple_junk ----

    #[test]
    fn apple_junk_detected() {
        for name in [
            "__MACOSX/foo",
            "__MACOSX",
            "__MACOSX/dir/._file",
            "a/._foo",
            "._x",
            "dir/.DS_Store",
            ".DS_Store",
            "Thumbs.db",
            "photos/thumbs.DB",
            "x\\._win",
        ] {
            assert!(is_apple_junk(name), "{name} should be junk");
        }
    }

    #[test]
    fn apple_junk_not_false_positive() {
        for name in ["desktop.ini", "a/desktop.ini", "docs/readme.txt", "a/.gitignore", "my._file", "Thumbs.dbx", "DS_Store"] {
            assert!(!is_apple_junk(name), "{name} should not be junk");
        }
    }

    // ---- unique_name ----

    #[test]
    fn unique_name_returns_same_when_free() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("free.txt");
        assert_eq!(unique_name(&p), p);
    }

    #[test]
    fn unique_name_appends_counter() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("a.txt");
        std::fs::write(&base, b"x").unwrap();
        assert_eq!(unique_name(&base), dir.path().join("a (1).txt"));

        std::fs::write(dir.path().join("a (1).txt"), b"x").unwrap();
        assert_eq!(unique_name(&base), dir.path().join("a (2).txt"));

        std::fs::write(dir.path().join("a (2).txt"), b"x").unwrap();
        assert_eq!(unique_name(&base), dir.path().join("a (3).txt"));
    }

    #[test]
    fn unique_name_without_extension() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("README");
        std::fs::write(&base, b"x").unwrap();
        assert_eq!(unique_name(&base), dir.path().join("README (1)"));
    }

    #[test]
    fn unique_name_multi_dot_keeps_last_extension() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("archive.tar.gz");
        std::fs::write(&base, b"x").unwrap();
        assert_eq!(unique_name(&base), dir.path().join("archive.tar (1).gz"));
    }

    // ---- ExcludeFilter ----

    fn filter(pats: &[&str]) -> ExcludeFilter {
        ExcludeFilter::new(pats.iter().copied())
    }

    #[test]
    fn exclude_empty_filter_matches_nothing() {
        let f = filter(&[]);
        assert!(f.is_empty());
        assert!(!f.matches("x", false));
        assert!(!ExcludeFilter::new([""]).matches("x", false));
        assert!(ExcludeFilter::new(["", "/"]).is_empty());
    }

    #[test]
    fn exclude_extension_glob_matches_any_component() {
        let f = filter(&["*.tmp"]);
        assert!(f.matches("x.tmp", false));
        assert!(f.matches("dir/x.tmp", false));
        assert!(f.matches("cache.tmp", true), "non-dir-only pattern matches dirs too");
        assert!(f.matches("x.tmp/inner.txt", false), "anything under a matching dir");
        assert!(!f.matches("x.txt", false));
        assert!(!f.matches("x.tmpx", false));
        assert!(!f.matches("tmp", false));
    }

    #[test]
    fn exclude_dir_only_pattern() {
        let f = filter(&["node_modules/"]);
        assert!(f.matches("node_modules", true));
        assert!(!f.matches("node_modules", false), "file named node_modules is not a dir");
        assert!(f.matches("a/node_modules", true));
        assert!(f.matches("node_modules/pkg/index.js", false));
        assert!(f.matches("src/node_modules/x.js", false));
        assert!(!f.matches("node_modules_x", true));
        assert!(!f.matches("node_modules_x/y", false));
    }

    #[test]
    fn exclude_question_mark() {
        let f = filter(&["?"]);
        assert!(f.matches("a", false));
        assert!(f.matches("dir/b", false));
        assert!(!f.matches("ab", false));
        assert!(!f.matches("", false));
    }

    #[test]
    fn exclude_star_matches_everything() {
        let f = filter(&["*"]);
        assert!(f.matches("anything", false));
        assert!(f.matches("a/b/c", true));
    }

    #[test]
    fn exclude_anchored_pattern() {
        let f = filter(&["docs/*.md"]);
        assert!(f.matches("docs/a.md", false));
        assert!(!f.matches("a/docs/a.md", false), "anchored at root");
        assert!(!f.matches("docs/sub/a.md", false));
        assert!(!f.matches("docs/a.txt", false));
    }

    #[test]
    fn exclude_leading_slash_is_anchored() {
        let f = filter(&["/build"]);
        assert!(f.matches("build", false));
        assert!(f.matches("build/out.o", false));
        assert!(!f.matches("x/build", false));
    }

    #[test]
    fn exclude_anchored_dir_only() {
        let f = filter(&["build/out/"]);
        assert!(f.matches("build/out", true));
        assert!(f.matches("build/out/x.o", false));
        assert!(!f.matches("build/out", false));
        assert!(!f.matches("x/build/out", true));
    }

    #[test]
    fn exclude_double_star() {
        let f = filter(&["**/cache"]);
        assert!(f.matches("cache", false));
        assert!(f.matches("a/b/cache", true));
        assert!(!f.matches("a/cachex", false));

        let g = filter(&["a/**"]);
        assert!(g.matches("a/x/y", false));
        assert!(!g.matches("b/x", false));
    }

    #[test]
    fn exclude_normalises_input_paths() {
        let f = filter(&["*.tmp", "docs/"]);
        assert!(f.matches("dir\\x.tmp", false));
        assert!(f.matches("./x.tmp", false));
        assert!(f.matches("/x.tmp", false));
        assert!(f.matches("docs\\a.txt", false));
        assert!(f.matches("./docs/a.txt", false));
    }

    #[test]
    fn exclude_multiple_patterns_any_match() {
        let f = filter(&["*.tmp", "*.bak", "node_modules/"]);
        assert!(f.matches("x.bak", false));
        assert!(f.matches("node_modules", true));
        assert!(!f.matches("x.rs", false));
    }

    #[test]
    fn glob_match_basics() {
        assert!(glob_match("*", ""));
        assert!(glob_match("a*c", "abbbc"));
        assert!(glob_match("a*c", "ac"));
        assert!(!glob_match("a*c", "abd"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*a*b*", "xxaxxbxx"));
        assert!(!glob_match("*a*b", "ba"));
        assert!(glob_match("**", "x"));
        assert!(glob_match("é?", "éx"));
    }
}
