//! Smart Extract planning (05 §1), format independent.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SmartPlan {
    /// Leading path component to strip from every entry (single root folder case).
    pub strip: Option<String>,
    /// Folder (named after the archive) to create and extract into.
    pub wrap: Option<String>,
}

/// `(name, is_dir)` pairs as stored in the archive.
pub fn plan(entries: &[(String, bool)], archive_stem: &str, wrap_single_files: bool) -> SmartPlan {
    let mut roots: Vec<(String, String, bool)> = Vec::new(); // (key, original, is_dir_root)
    let mut any = false;
    for (name, is_dir) in entries {
        let n = name.replace('\\', "/");
        let mut n = n.as_str();
        while let Some(r) = n.strip_prefix("./") { n = r; }
        let n = n.trim_start_matches('/');
        if n.is_empty() { continue; }
        any = true;
        let (first, rest) = match n.split_once('/') { Some((f, r)) => (f, !r.is_empty()), None => (n, false) };
        let root_is_dir = rest || *is_dir;
        let key = first.to_lowercase();
        if let Some(r) = roots.iter_mut().find(|r| r.0 == key) { r.2 |= root_is_dir; }
        else { roots.push((key, first.to_string(), root_is_dir)); }
    }
    if !any { return SmartPlan::default(); }
    if roots.len() == 1 {
        let (_, orig, is_dir) = &roots[0];
        if *is_dir { return SmartPlan { strip: Some(orig.clone()), wrap: None }; }
        if !wrap_single_files { return SmartPlan::default(); }
    }
    SmartPlan { strip: None, wrap: Some(archive_stem.to_string()) }
}

/// Archive name without archive extension(s): `a.tar.gz` -> `a`, `x.7z.001` -> `x`.
pub fn archive_stem(file_name: &str) -> String {
    let mut s = file_name.to_string();
    let lower = |s: &str| s.to_lowercase();
    // split-volume suffixes
    if let Some((base, ext)) = s.rsplit_once('.') {
        let e = lower(ext);
        if (e.len() == 3 && e.chars().all(|c| c.is_ascii_digit())) || (e.starts_with('z') && e.len() == 3 && e[1..].chars().all(|c| c.is_ascii_digit())) {
            s = base.to_string();
        }
    }
    let l = lower(&s);
    for suf in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.lz4", ".tar.br", ".tar.lzma"] {
        if l.ends_with(suf) { return s[..s.len() - suf.len()].to_string(); }
    }
    match s.rsplit_once('.') { Some((b, _)) if !b.is_empty() => b.to_string(), _ => s }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn e(v: &[(&str, bool)]) -> Vec<(String, bool)> { v.iter().map(|(a, b)| (a.to_string(), *b)).collect() }
    #[test] fn single_root() {
        let p = plan(&e(&[("proj/", true), ("proj/a.txt", false), ("proj/sub/b", false)]), "arc", false);
        assert_eq!(p, SmartPlan { strip: Some("proj".into()), wrap: None });
    }
    #[test] fn single_root_no_dir_entry() {
        assert_eq!(plan(&e(&[("proj/a.txt", false)]), "arc", false).strip.as_deref(), Some("proj"));
    }
    #[test] fn single_file() {
        assert_eq!(plan(&e(&[("a.txt", false)]), "arc", false), SmartPlan::default());
        assert_eq!(plan(&e(&[("a.txt", false)]), "arc", true).wrap.as_deref(), Some("arc"));
    }
    #[test] fn multi_root() {
        assert_eq!(plan(&e(&[("a", false), ("b", false)]), "arc", false).wrap.as_deref(), Some("arc"));
    }
    #[test] fn root_plus_stray() {
        assert_eq!(plan(&e(&[("d/x", false), ("stray.txt", false)]), "arc", false).wrap.as_deref(), Some("arc"));
    }
    #[test] fn dot_slash_and_backslash() {
        assert_eq!(plan(&e(&[("./d/x", false), ("d\\y", false)]), "arc", false).strip.as_deref(), Some("d"));
    }
    #[test] fn empty() { assert_eq!(plan(&[], "arc", false), SmartPlan::default()); }
    #[test] fn stems() {
        assert_eq!(archive_stem("a.tar.gz"), "a");
        assert_eq!(archive_stem("x.7z.001"), "x");
        assert_eq!(archive_stem("My Docs.zip"), "My Docs");
        assert_eq!(archive_stem("v.z01"), "v");
    }
}
