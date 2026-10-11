//! ZIP LZMA (write+read) and Deflate64 (read) interoperability with the system 7-Zip.
use fz_core::enumerate::{enumerate, EnumOptions};
use fz_core::zip::{create_zip, test_archive, Encryption, Level, Method, ReadOptions, ZipArchive, ZipOptions};
use fz_core::{CancelToken, Progress};
use std::fs;
use std::process::Command;

fn have_7z() -> bool { Command::new("7z").arg("i").output().is_ok() }

fn tree(d: &std::path::Path) -> std::path::PathBuf {
    let s = d.join("src");
    fs::create_dir_all(&s).unwrap();
    fs::write(s.join("a.txt"), "lorem ipsum ".repeat(20000)).unwrap();
    fs::write(s.join("tiny"), "x").unwrap();
    s
}

fn roundtrip(method: Method, enc: Encryption, pw: Option<&str>) {
    let d = tempfile::tempdir().unwrap();
    let s = tree(d.path());
    let c = CancelToken::new();
    let mut eo = EnumOptions::default();
    eo.wrap_single_root = false;
    let entries = enumerate(&[s], &eo, &c).unwrap();
    let o = ZipOptions { method, level: Level::Normal, encryption: enc, ..Default::default() };
    let out = d.path().join("o.zip");
    create_zip(&out, &entries, &o, &c, &Progress::new()).unwrap();
    let a = ZipArchive::open(&out, ReadOptions::default()).unwrap();
    assert!(test_archive(&a, pw, &c, &Progress::new()).unwrap().iter().all(|(n, r)| r.is_ok() || { eprintln!("{n}: {r:?}"); false }));
    if have_7z() {
        let mut cmd = Command::new("7z");
        cmd.arg("t").arg(&out);
        if let Some(p) = pw { cmd.arg(format!("-p{p}")); }
        assert!(cmd.output().unwrap().status.success(), "7z rejected {method:?}");
    }
}

#[test] fn lzma_plain_is_readable_by_7zip_and_ourselves() { roundtrip(Method::Lzma, Encryption::None, None); }
#[test] fn ppmd_plain_is_readable_by_7zip_and_ourselves() { roundtrip(Method::Ppmd, Encryption::None, None); }
#[test] fn ppmd_with_aes() { roundtrip(Method::Ppmd, Encryption::Aes { strength: 2, password: "pw".into() }, Some("pw")); }
#[test] fn lzma_with_aes() { roundtrip(Method::Lzma, Encryption::Aes { strength: 3, password: "pw".into() }, Some("pw")); }

#[test]
fn reads_deflate64_and_lzma_archives_made_by_7zip() {
    if !have_7z() { return; }
    for m in ["Deflate64", "LZMA", "BZip2", "PPMd"] {
        let d = tempfile::tempdir().unwrap();
        let s = tree(d.path());
        let z = d.path().join("m.zip");
        let st = Command::new("7z").current_dir(d.path()).args(["a", "-tzip", &format!("-mm={m}"), "m.zip", "src"]).output().unwrap();
        assert!(st.status.success());
        let a = ZipArchive::open(&z, ReadOptions::default()).unwrap();
        let res = test_archive(&a, None, &CancelToken::new(), &Progress::new()).unwrap();
        assert!(res.iter().all(|(_, r)| r.is_ok()), "{m}: {res:?}");
        drop(s);
    }
}
