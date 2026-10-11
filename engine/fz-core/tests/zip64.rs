//! ZIP64 triggers: >65535 entries (always run) and >4 GB file (ignored: needs ~4.5 GB disk, ~40 s).
use fz_core::enumerate::{enumerate, EnumOptions};
use fz_core::zip::{create_zip, test_archive, ReadOptions, ZipArchive, ZipOptions};
use fz_core::{CancelToken, Progress};
use std::fs;

fn sig_count(path: &std::path::Path, sig: &[u8]) -> usize {
    let d = fs::read(path).unwrap();
    d.windows(sig.len()).filter(|w| *w == sig).count()
}

#[test]
fn more_than_65535_entries_use_zip64_end_records_and_read_back() {
    let d = tempfile::tempdir().unwrap();
    let s = d.path().join("many");
    fs::create_dir_all(&s).unwrap();
    for i in 0..70_000u32 { fs::write(s.join(format!("f{i}")), [b'a'; 3]).unwrap(); }
    let c = CancelToken::new();
    let mut eo = EnumOptions::default();
    eo.wrap_single_root = false;
    let entries = enumerate(&[s], &eo, &c).unwrap();
    let out = d.path().join("m.zip");
    create_zip(&out, &entries, &ZipOptions::default(), &c, &Progress::new()).unwrap();
    assert!(sig_count(&out, b"PK\x06\x06") >= 1 && sig_count(&out, b"PK\x06\x07") >= 1, "zip64 EOCD + locator expected");
    let a = ZipArchive::open(&out, ReadOptions::default()).unwrap();
    assert_eq!(a.entries.len(), 70_000);
    assert!(test_archive(&a, None, &c, &Progress::new()).unwrap().iter().all(|(_, r)| r.is_ok()));
    if let Ok(o) = std::process::Command::new("unzip").arg("-tq").arg(&out).output() { assert!(o.status.success()); }
}

#[test]
#[ignore = "heavy: writes a 4.5 GB archive; run with --ignored"]
fn file_over_4gb_streams_with_zip64_and_data_descriptor() {
    let d = tempfile::tempdir().unwrap();
    let big = d.path().join("big.dat");
    fs::File::create(&big).unwrap().set_len(4_500_000_000).unwrap();
    let c = CancelToken::new();
    let mut eo = EnumOptions::default();
    eo.wrap_single_root = false;
    let entries = enumerate(&[big], &eo, &c).unwrap();
    let out = d.path().join("z.zip");
    create_zip(&out, &entries, &ZipOptions::default(), &c, &Progress::new()).unwrap();
    assert!(sig_count(&out, b"PK\x06\x06") >= 1);
    let a = ZipArchive::open(&out, ReadOptions::default()).unwrap();
    assert_eq!(a.entries[0].size, 4_500_000_000);
    assert!(test_archive(&a, None, &c, &Progress::new()).unwrap().iter().all(|(_, r)| r.is_ok()));
}
