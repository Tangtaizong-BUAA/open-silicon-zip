//! Skip list: file extensions that are stored (not compressed) in an archive.
//!
//! Compressing already-compressed media, archives and images wastes CPU for no
//! size gain, so writers consult this list and store matching entries.
use std::collections::BTreeSet;

/// Default extensions stored without compression. All lowercase, no leading dot.
pub const DEFAULT_SKIP_EXTS: &[&str] = &[
    // archives
    "7z", "xz", "lzma", "ace", "arc", "arj", "bz", "tbz", "bz2", "tbz2", "cab", "deb", "gz", "tgz",
    "ha", "lha", "lzh", "lzo", "lzx", "pak", "rar", "rpm", "sit", "zoo", "zip", "jar", "ear", "war",
    "msi", "zst", "br", "lz4", "zpaq", "egg", "alz",
    // media
    "3gp", "avi", "mov", "mpeg", "mpg", "mpe", "wmv", "aac", "ape", "fla", "flac", "la", "mp3",
    "m4a", "mp4", "ofr", "ogg", "pac", "ra", "rm", "rka", "shn", "swa", "tta", "wv", "wma", "wav",
    "swf",
    // images
    "gif", "jpeg", "jpg", "jp2", "png", "tiff", "bmp", "ico", "psd", "psp",
    // documents
    "pdf", "doc", "docx", "dot", "rtf", "xls", "xlr", "xlt", "xlw", "ppt", "sxc", "sxd", "sxi", "sxg",
    "sxw", "odt", "ott", "odg", "otg", "odp", "otp", "ods", "ots", "odf",
    // binaries
    "exe", "dll", "ocx", "com", "obj", "lib", "so", "dylib", "sfx", "sys", "tlb", "class",
    // disk images
    "iso", "bin", "nrg", "mdf", "img", "vfd", "vhd", "vmdk", "dsk", "nvram", "vmem",
    // fonts
    "awg", "ps", "eps", "ai", "otf", "ttf", "fon",
    // modern formats
    "pptx", "xlsx", "webp", "heic", "avif", "mkv",
];

/// Set of extensions whose entries are stored instead of compressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkipList {
    exts: BTreeSet<String>,
}

/// High-speed (store-first) mode for the writer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HighSpeedMode {
    #[default]
    Off,
    On,
    Aggressive,
}

impl Default for SkipList {
    fn default() -> Self {
        Self::from_exts(DEFAULT_SKIP_EXTS.iter().map(|e| (*e).to_string()))
    }
}

impl SkipList {
    /// Builds a list from arbitrary extensions. Leading dots and case are normalised;
    /// empty entries are ignored.
    pub fn from_exts(exts: impl IntoIterator<Item = String>) -> Self {
        let mut list = SkipList { exts: BTreeSet::new() };
        for ext in exts {
            list.add(&ext);
        }
        list
    }

    /// True if `name`'s extension (case-insensitive, last path component only) is in the list.
    /// Names without an extension return false.
    pub fn should_store(&self, name: &str) -> bool {
        match extension_of(name) {
            Some(ext) => self.exts.contains(&ext),
            None => false,
        }
    }

    /// Adds an extension. Returns false if it was empty or already present.
    pub fn add(&mut self, ext: &str) -> bool {
        match normalize_ext(ext) {
            Some(e) => self.exts.insert(e),
            None => false,
        }
    }

    /// Removes an extension. Returns true if it was present.
    pub fn remove(&mut self, ext: &str) -> bool {
        match normalize_ext(ext) {
            Some(e) => self.exts.remove(&e),
            None => false,
        }
    }

    /// All extensions, sorted.
    pub fn to_vec(&self) -> Vec<String> {
        self.exts.iter().cloned().collect()
    }
}

fn normalize_ext(ext: &str) -> Option<String> {
    let e = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if e.is_empty() { None } else { Some(e) }
}

/// Lowercased extension of the last path component, or None.
/// A leading dot (".mp3") does not start an extension, matching `Path::extension`.
fn extension_of(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let dot = base.rfind('.')?;
    if dot == 0 {
        return None;
    }
    let ext = &base[dot + 1..];
    if ext.is_empty() { None } else { Some(ext.to_ascii_lowercase()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_list_has_exact_contents() {
        assert_eq!(DEFAULT_SKIP_EXTS.len(), 133);
        let list = SkipList::default();
        assert_eq!(list.to_vec().len(), 133, "default list must not contain duplicates");
    }

    #[test]
    fn default_exts_are_lowercase_and_unique() {
        let mut seen = BTreeSet::new();
        for e in DEFAULT_SKIP_EXTS {
            assert_eq!(*e, e.to_ascii_lowercase(), "{e} not lowercase");
            assert!(!e.starts_with('.'), "{e} has leading dot");
            assert!(seen.insert(*e), "duplicate {e}");
        }
    }

    #[test]
    fn default_contains_representative_exts() {
        let list = SkipList::default();
        for e in ["zip", "7z", "mp4", "flac", "jpg", "pdf", "docx", "exe", "iso", "ttf", "pptx", "mkv", "avif"] {
            assert!(list.to_vec().contains(&e.to_string()), "missing {e}");
        }
        assert!(!list.to_vec().contains(&"txt".to_string()));
        assert!(!list.to_vec().contains(&"rs".to_string()));
    }

    #[test]
    fn should_store_matches_extension_case_insensitively() {
        let list = SkipList::default();
        assert!(list.should_store("photo.JPG"));
        assert!(list.should_store("dir/photo.jpg"));
        assert!(list.should_store("dir\\sub\\movie.MKV"));
        assert!(list.should_store("archive.tar.gz"));
        assert!(!list.should_store("notes.txt"));
    }

    #[test]
    fn should_store_no_extension_is_false() {
        let list = SkipList::default();
        assert!(!list.should_store("README"));
        assert!(!list.should_store("dir/Makefile"));
        assert!(!list.should_store(".mp3"), "leading-dot name has no extension");
        assert!(!list.should_store("trailing."));
        assert!(!list.should_store(""));
    }

    #[test]
    fn extension_only_taken_from_last_component() {
        let list = SkipList::default();
        assert!(!list.should_store("zip.d/readme"));
        assert!(!list.should_store("photos.jpg/"));
    }

    #[test]
    fn add_and_remove() {
        let mut list = SkipList::from_exts(Vec::<String>::new());
        assert!(!list.should_store("a.xyz"));
        assert!(list.add(".XYZ"));
        assert!(!list.add("xyz"), "duplicate add reports false");
        assert!(list.should_store("a.xyz"));
        assert!(!list.add("  "), "empty ext ignored");
        assert_eq!(list.to_vec(), vec!["xyz".to_string()]);
        assert!(list.remove("Xyz"));
        assert!(!list.remove("xyz"));
        assert!(!list.should_store("a.xyz"));
    }

    #[test]
    fn removing_default_ext_disables_it() {
        let mut list = SkipList::default();
        assert!(list.remove("zip"));
        assert!(!list.should_store("data.zip"));
        assert!(list.should_store("data.7z"));
    }

    #[test]
    fn to_vec_is_sorted() {
        let list = SkipList::from_exts(["zz", "aa", "mm"].iter().map(|s| s.to_string()));
        assert_eq!(list.to_vec(), vec!["aa", "mm", "zz"]);
    }

    #[test]
    fn from_exts_is_independent_of_default() {
        let list = SkipList::from_exts(vec!["foo".to_string()]);
        assert!(list.should_store("x.foo"));
        assert!(!list.should_store("x.zip"));
    }

    #[test]
    fn high_speed_mode_default_is_off() {
        assert_eq!(HighSpeedMode::default(), HighSpeedMode::Off);
        assert_ne!(HighSpeedMode::On, HighSpeedMode::Aggressive);
    }
}
