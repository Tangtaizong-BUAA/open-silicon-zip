//! ZIP editing (FR-301..303), repair (FR-702) and checksum tool (FR-704) tests.
use fz_core::enumerate::{enumerate, EnumOptions};
use fz_core::zip::edit::{edit_zip, EditOp};
use fz_core::zip::repair::repair_zip;
use fz_core::zip::{create_zip, test_archive, ReadOptions, ZipArchive, ZipOptions};
use fz_core::{CancelToken, ErrorKind, Progress};
use std::fs;

fn make(dir: &std::path::Path) -> std::path::PathBuf {
    let src = dir.join("in");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("a.txt"), "alpha ".repeat(5000)).unwrap();
    fs::write(src.join("b.txt"), "beta").unwrap();
    fs::write(src.join("sub/c.txt"), "gamma ".repeat(100)).unwrap();
    let c = CancelToken::new();
    let mut eo = EnumOptions::default();
    eo.wrap_single_root = false;
    let entries = enumerate(&[src], &eo, &c).unwrap();
    let out = dir.join("t.zip");
    create_zip(&out, &entries, &ZipOptions::default(), &c, &Progress::new()).unwrap();
    out
}

fn names(p: &std::path::Path) -> Vec<String> {
    let mut v: Vec<_> = ZipArchive::open(p, ReadOptions::default()).unwrap().entries.iter().map(|e| e.name.clone()).collect();
    v.sort();
    v
}

#[test]
fn delete_rename_add_keep_untouched_entries_byte_identical() {
    let d = tempfile::tempdir().unwrap();
    let z = make(d.path());
    let before = ZipArchive::open(&z, ReadOptions::default()).unwrap();
    let idx = |n: &str| before.entries.iter().find(|e| e.name == n).unwrap().index;
    let extra = d.path().join("new.txt");
    fs::write(&extra, "fresh").unwrap();
    let c = CancelToken::new();
    let mut eo = EnumOptions::default();
    eo.wrap_single_root = false;
    let add = enumerate(&[extra], &eo, &c).unwrap();
    let op = EditOp { delete: vec![idx("b.txt")], rename: vec![(idx("a.txt"), "renamed/ä中.txt".into())], add };
    edit_zip(&z, &op, &ZipOptions::default(), &c, &Progress::new()).unwrap();
    let n = names(&z);
    assert!(n.contains(&"renamed/ä中.txt".to_string()) && n.contains(&"new.txt".to_string()) && !n.contains(&"b.txt".to_string()), "{n:?}");
    let a = ZipArchive::open(&z, ReadOptions::default()).unwrap();
    assert!(test_archive(&a, None, &c, &Progress::new()).unwrap().iter().all(|(_, r)| r.is_ok()));
    // untouched entry sub/c.txt: same crc/size
    let old = before.entries.iter().find(|e| e.name == "sub/c.txt").unwrap();
    let new = a.entries.iter().find(|e| e.name == "sub/c.txt").unwrap();
    assert_eq!((old.crc, old.csize, old.size), (new.crc, new.csize, new.size));
    let mut out = Vec::new();
    a.read_entry(a.entries.iter().find(|e| e.name == "renamed/ä中.txt").unwrap(), None, &mut out).unwrap();
    assert_eq!(out, "alpha ".repeat(5000).into_bytes());
    // system unzip agrees when available
    if let Ok(o) = std::process::Command::new("unzip").arg("-tq").arg(&z).output() { assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout)); }
}

#[test]
fn rename_to_invalid_or_duplicate_name_is_rejected_and_archive_untouched() {
    let d = tempfile::tempdir().unwrap();
    let z = make(d.path());
    let orig = fs::read(&z).unwrap();
    let a = ZipArchive::open(&z, ReadOptions::default()).unwrap();
    let i = a.entries.iter().find(|e| e.name == "a.txt").unwrap().index;
    let c = CancelToken::new();
    for bad in ["", "x\\y", "C:foo", "../up", "b.txt"] {
        let e = edit_zip(&z, &EditOp { rename: vec![(i, bad.into())], ..Default::default() }, &ZipOptions::default(), &c, &Progress::new()).unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidInput, "{bad}");
    }
    assert_eq!(fs::read(&z).unwrap(), orig);
}

#[test]
fn adding_same_name_replaces_entry() {
    let d = tempfile::tempdir().unwrap();
    let z = make(d.path());
    let f = d.path().join("b.txt");
    fs::write(&f, "replaced!").unwrap();
    let c = CancelToken::new();
    let mut eo = EnumOptions::default();
    eo.wrap_single_root = false;
    let add = enumerate(&[f], &eo, &c).unwrap();
    edit_zip(&z, &EditOp { add, ..Default::default() }, &ZipOptions::default(), &c, &Progress::new()).unwrap();
    let a = ZipArchive::open(&z, ReadOptions::default()).unwrap();
    assert_eq!(a.entries.iter().filter(|e| e.name == "b.txt").count(), 1);
    let mut out = Vec::new();
    a.read_entry(a.entries.iter().find(|e| e.name == "b.txt").unwrap(), None, &mut out).unwrap();
    assert_eq!(out, b"replaced!");
}

#[test]
fn repair_rebuilds_a_truncated_central_directory() {
    let d = tempfile::tempdir().unwrap();
    let z = make(d.path());
    let mut bytes = fs::read(&z).unwrap();
    bytes.truncate(bytes.len() - 60); // cut into the central directory / EOCD
    let bad = d.path().join("bad.zip");
    fs::write(&bad, &bytes).unwrap();
    assert!(ZipArchive::open(&bad, ReadOptions::default()).is_err());
    let fixed = d.path().join("fixed.zip");
    let rep = repair_zip(&bad, &fixed, &CancelToken::new()).unwrap();
    assert_eq!(rep.recovered.len(), 4, "{rep:?}");
    assert!(rep.lost.is_empty());
    let a = ZipArchive::open(&fixed, ReadOptions::default()).unwrap();
    assert!(test_archive(&a, None, &CancelToken::new(), &Progress::new()).unwrap().iter().all(|(_, r)| r.is_ok()));
    assert!(bad.exists() && fs::read(&bad).unwrap() == bytes, "input must never be modified");
}

#[test]
fn repair_reports_truncated_entry_and_keeps_the_rest() {
    let d = tempfile::tempdir().unwrap();
    let z = make(d.path());
    let a = ZipArchive::open(&z, ReadOptions::default()).unwrap();
    let last = a.entries.iter().max_by_key(|e| e.local_off).unwrap();
    let cut = (last.local_off + 30 + last.name.len() as u64 + 5) as usize;
    let mut bytes = fs::read(&z).unwrap();
    bytes.truncate(cut);
    let bad = d.path().join("bad2.zip");
    fs::write(&bad, bytes).unwrap();
    let fixed = d.path().join("fixed2.zip");
    let rep = repair_zip(&bad, &fixed, &CancelToken::new()).unwrap();
    assert_eq!(rep.recovered.len() + rep.lost.len() + rep.partial.len(), 4, "{rep:?}");
    assert!(!rep.lost.is_empty() || !rep.partial.is_empty());
    let a2 = ZipArchive::open(&fixed, ReadOptions::default()).unwrap();
    assert!(a2.entries.len() >= 2);
}

#[test]
fn checksum_tool_matches_known_vectors() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("v");
    fs::write(&f, "123456789").unwrap();
    let h = fz_core::hash::digest_file(&f).unwrap();
    assert_eq!(h.crc32, "cbf43926");
    assert_eq!(h.crc64, "995dc9bbdf1939fa");
    assert_eq!(h.md5, "25f9e794323b453885f5181f1b624d0b");
    assert_eq!(h.sha1, "f7c3bc1d808e04732adf679965ccc34ca7ae3441");
    assert_eq!(h.sha256, "15e2b0d3c33891ebb0f1ef609ec419420c20e320ce94c65fbc8c3312448eb225");
    assert_eq!(fz_core::hash::compare_expected(&h, "CBF43926"), fz_core::hash::MatchKind::Full);
    assert_eq!(fz_core::hash::compare_expected(&h, &h.sha256[..10]), fz_core::hash::MatchKind::Partial);
    assert_eq!(fz_core::hash::compare_expected(&h, "deadbeef"), fz_core::hash::MatchKind::Mismatch);
}
