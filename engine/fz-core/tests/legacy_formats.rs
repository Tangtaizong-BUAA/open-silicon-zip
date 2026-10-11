//! Legacy read-side formats through the libarchive adapter (spec 03 §1) plus the native UUE/XXE/Base64 decoder.
//! Fixtures are generated at test time with system tools; a case is skipped (with a note) when its tool is missing.
#![cfg(feature = "legacy")]
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use fz_core::formats::{self, detect, Format};
use fz_core::zip::ExtractOptions;
use fz_core::{CancelToken, ErrorKind, Progress};
use tempfile::TempDir;

// ------------------------------------------------------------------ helpers
fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().map(|o| o.status.success()).unwrap_or(false)
}
macro_rules! need {
    ($($t:expr),+) => { $( if !have($t) { eprintln!("SKIP: tool `{}` not installed", $t); return; } )+ };
}
fn run(dir: &Path, cmd: &str) {
    let o = Command::new("sh").arg("-c").arg(cmd).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "`{cmd}` failed: {}", String::from_utf8_lossy(&o.stderr));
}
fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n).map(|_| { s ^= s << 13; s ^= s >> 7; s ^= s << 17; (s >> 24) as u8 }).collect()
}
fn write(p: &Path, d: &[u8]) { fs::create_dir_all(p.parent().unwrap()).unwrap(); fs::write(p, d).unwrap(); }

/// Source tree: text, a >256 KB binary (exercises chunking), nested dirs, an empty file, a symlink.
fn make_tree(root: &Path, symlink: bool) {
    write(&root.join("a.txt"), b"hello legacy world\n");
    write(&root.join("sub/b.bin"), &noise(700_000, 7));
    write(&root.join("sub/deep/c.txt"), "unicode \u{e9}\u{4e2d}\n".as_bytes());
    write(&root.join("empty.dat"), b"");
    fs::create_dir_all(root.join("emptydir")).unwrap();
    #[cfg(unix)]
    if symlink { std::os::unix::fs::symlink("a.txt", root.join("link.txt")).unwrap(); }
    let _ = symlink;
}

fn collect(root: &Path) -> Vec<(String, Option<Vec<u8>>, Option<String>)> {
    fn walk(base: &Path, cur: &Path, out: &mut Vec<(String, Option<Vec<u8>>, Option<String>)>) {
        let mut es: Vec<_> = fs::read_dir(cur).unwrap().map(|e| e.unwrap()).collect();
        es.sort_by_key(|e| e.file_name());
        for e in es {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            let ft = fs::symlink_metadata(&p).unwrap().file_type();
            if ft.is_symlink() { out.push((rel, None, Some(fs::read_link(&p).unwrap().to_string_lossy().into_owned()))); }
            else if ft.is_dir() { out.push((format!("{rel}/"), None, None)); walk(base, &p, out); }
            else { out.push((rel, Some(fs::read(&p).unwrap()), None)); }
        }
    }
    let mut v = Vec::new();
    walk(root, root, &mut v);
    v
}

fn opts(dest: &Path) -> ExtractOptions {
    let mut o = ExtractOptions::new(dest);
    o.smart = false;
    o
}

fn extract(archive: &Path, dest: &Path) -> fz_core::zip::extract::ExtractReport {
    formats::extract(archive, &opts(dest), None, &CancelToken::new(), &Progress::new()).expect("extract")
}

fn assert_tree_eq(src: &Path, dst: &Path) {
    let a = collect(src);
    let b = collect(dst);
    // case-insensitive: LHA tools lower-case MS-DOS names, libarchive keeps the stored (upper-case) spelling
    let names = |v: &Vec<(String, Option<Vec<u8>>, Option<String>)>| v.iter().map(|x| x.0.to_lowercase()).collect::<Vec<_>>();
    assert_eq!(names(&a), names(&b), "tree listing differs");
    for (x, y) in a.iter().zip(&b) { assert_eq!((&x.1, &x.2), (&y.1, &y.2), "content differs for {}", x.0); }
}

/// detect + list + test + extract byte-compare for one archive made from `src`.
fn check_roundtrip(archive: &Path, src: &Path, expect_label: &str) {
    assert_eq!(detect(archive).unwrap(), Format::Legacy, "{expect_label}: detect");
    let items = formats::list(archive, None, None, &CancelToken::new()).expect("list");
    assert!(items.iter().any(|i| i.name.trim_end_matches('/').ends_with("a.txt")), "{expect_label}: listing lacks a.txt: {:?}", items.iter().map(|i| &i.name).collect::<Vec<_>>());
    let big = items.iter().find(|i| i.name.ends_with("b.bin")).expect("b.bin listed");
    assert_eq!(big.size, 700_000, "{expect_label}: size");
    let t = formats::test(archive, None, &CancelToken::new(), &Progress::new()).expect("test");
    assert!(!t.is_empty() && t.iter().all(|(n, r)| r.is_ok()), "{expect_label}: test results {:?}", t.iter().filter(|x| x.1.is_err()).map(|x| (&x.0, x.1.as_ref().unwrap_err().to_string())).collect::<Vec<_>>());
    let out = TempDir::new().unwrap();
    let rep = extract(archive, out.path());
    assert!(rep.unsafe_refused.is_empty() && rep.failed.is_empty(), "{expect_label}: {rep:?}");
    assert_tree_eq(src, out.path());
}

// ------------------------------------------------------------------ real-tool fixtures
#[test]
fn cpio_newc_and_odc() {
    need!("cpio");
    for (fmt, name) in [("newc", "t.cpio"), ("odc", "t2.cpio"), ("crc", "t3.cpio")] {
        let w = TempDir::new().unwrap();
        let src = w.path().join("src");
        make_tree(&src, true);
        let o = Command::new("sh")
            .arg("-c")
            .arg(format!("find . -mindepth 1 | sort | cpio -o -H {fmt} --quiet > ../{name}"))
            .current_dir(&src)
            .output()
            .unwrap();
        if o.status.success() {
            check_roundtrip(&w.path().join(name), &src, &format!("cpio {fmt}"));
        } else {
            // macOS bsdcpio has no `-H crc`; emit the crc ("sv4cpio") archive ourselves so the
            // format stays covered on both platforms.
            assert_eq!(fmt, "crc", "unexpected cpio failure: {}", String::from_utf8_lossy(&o.stderr));
            fs::write(w.path().join(name), cpio_sv4_bytes(&src, true)).unwrap();
            check_roundtrip(&w.path().join(name), &src, "cpio crc (rust fixture)");
        }
    }
}

/// Minimal `newc`-family cpio writer (magic 070701, or 070702 with the header-word checksum).
fn cpio_sv4_bytes(root: &Path, crc: bool) -> Vec<u8> {
    fn pad4(v: &mut Vec<u8>) {
        while v.len() % 4 != 0 {
            v.push(0);
        }
    }
    fn entry(out: &mut Vec<u8>, name: &str, mode: u32, size: u32, data: &[u8], crc: bool) {
        let h = |x: u32| format!("{:08x}", x);
        let mut hdr = String::with_capacity(110);
        hdr.push_str(if crc { "070702" } else { "070701" });
        hdr.push_str(&h(1)); // ino
        hdr.push_str(&h(mode));
        hdr.push_str(&h(0)); // uid
        hdr.push_str(&h(0)); // gid
        hdr.push_str(&h(1)); // nlink
        hdr.push_str(&h(0)); // mtime
        hdr.push_str(&h(size));
        for _ in 0..4 {
            hdr.push_str(&h(0)); // devmajor, devminor, rdevmajor, rdevminor
        }
        hdr.push_str(&h(name.len() as u32 + 1)); // namesize incl. NUL
        hdr.push_str(&h(0)); // check, filled below
        debug_assert_eq!(hdr.len(), 110);
        let mut check: u32 = 0;
        if crc {
            let mut w = [0u8; 4];
            for c in hdr.as_bytes().chunks(4) {
                w = [0u8; 4];
                w[..c.len()].copy_from_slice(c);
                check = check.wrapping_add(u32::from_be_bytes(w));
            }
        }
        out.extend_from_slice(hdr.as_bytes());
        let check_at = out.len() - 8;
        out[check_at..check_at + 8].copy_from_slice(format!("{:08x}", check).as_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        pad4(out);
        out.extend_from_slice(data);
        pad4(out);
    }
    fn walk(base: &Path, cur: &Path, out: &mut Vec<(String, PathBuf)>) {
        let mut es: Vec<_> = fs::read_dir(cur).unwrap().map(|e| e.unwrap()).collect();
        es.sort_by_key(|e| e.file_name());
        for e in es {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            let ft = fs::symlink_metadata(&p).unwrap().file_type();
            if ft.is_dir() {
                out.push((format!("{rel}/"), p.clone()));
                walk(base, &p, out);
            } else {
                out.push((rel, p));
            }
        }
    }
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    walk(root, root, &mut files);
    let mut out = Vec::new();
    for (rel, p) in &files {
        let ft = fs::symlink_metadata(p).unwrap().file_type();
        if ft.is_symlink() {
            let t = fs::read_link(p).unwrap().to_string_lossy().into_owned().into_bytes();
            entry(&mut out, rel, 0o120777, t.len() as u32, &t, crc);
        } else if ft.is_dir() {
            entry(&mut out, rel, 0o40755, 0, b"", crc);
        } else {
            let data = fs::read(p).unwrap();
            entry(&mut out, rel, 0o100644, data.len() as u32, &data, crc);
        }
    }
    entry(&mut out, "TRAILER!!!", 0, 0, b"", crc);
    pad4(&mut out);
    out
}

#[test]
fn ar_archive() {
    need!("ar");
    let w = TempDir::new().unwrap();
    let src = w.path().join("src");
    write(&src.join("one.o"), &noise(300_000, 3));
    write(&src.join("two.txt"), b"second member\n");
    run(&src, "ar rc ../lib.a one.o two.txt");
    let a = w.path().join("lib.a");
    assert_eq!(detect(&a).unwrap(), Format::Legacy);
    let out = TempDir::new().unwrap();
    extract(&a, out.path());
    assert_eq!(fs::read(out.path().join("one.o")).unwrap(), fs::read(src.join("one.o")).unwrap());
    assert_eq!(fs::read(out.path().join("two.txt")).unwrap(), b"second member\n");
}

#[test]
fn deb_lists_and_extracts_data_payload() {
    need!("dpkg-deb");
    let w = TempDir::new().unwrap();
    let pkg = w.path().join("pkg");
    write(&pkg.join("DEBIAN/control"), b"Package: fztest\nVersion: 1.0\nArchitecture: all\nMaintainer: t <t@example.com>\nDescription: t\n");
    write(&pkg.join("usr/share/fztest/hello.txt"), b"hello deb\n");
    write(&pkg.join("usr/share/fztest/big.bin"), &noise(400_000, 11));
    let deb = w.path().join("fztest.deb");
    run(w.path(), &format!("dpkg-deb --build -Znone pkg {} >/dev/null 2>&1 || dpkg-deb --build pkg {} >/dev/null", deb.display(), deb.display()));
    assert_eq!(detect(&deb).unwrap(), Format::Legacy);
    let items = formats::list(&deb, None, None, &CancelToken::new()).unwrap();
    let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
    assert!(names.contains(&"usr/share/fztest/hello.txt"), "{names:?}");
    assert!(!names.iter().any(|n| n.contains("control")), "control.tar must not be listed: {names:?}");
    let out = TempDir::new().unwrap();
    extract(&deb, out.path());
    assert_eq!(fs::read(out.path().join("usr/share/fztest/hello.txt")).unwrap(), b"hello deb\n");
    assert_eq!(fs::read(out.path().join("usr/share/fztest/big.bin")).unwrap(), fs::read(pkg.join("usr/share/fztest/big.bin")).unwrap());
    assert!(formats::test(&deb, None, &CancelToken::new(), &Progress::new()).unwrap().iter().all(|x| x.1.is_ok()));
}

#[test]
fn iso9660_with_rockridge() {
    need!("bsdtar");
    let w = TempDir::new().unwrap();
    let src = w.path().join("src");
    make_tree(&src, false);
    let o = Command::new("bsdtar").current_dir(&src).args(["--format", "iso9660", "-cf", "../t.iso", "."]).output().unwrap();
    if !o.status.success() { eprintln!("SKIP: bsdtar cannot write iso9660"); return; }
    let iso = w.path().join("t.iso");
    assert_eq!(detect(&iso).unwrap(), Format::Legacy);
    let items = formats::list(&iso, None, None, &CancelToken::new()).unwrap();
    assert!(items.iter().any(|i| i.name.ends_with("b.bin") && i.size == 700_000), "{:?}", items.iter().map(|i| &i.name).collect::<Vec<_>>());
    // FR-701: the test operation walks every file's data and reports per-entry results
    let t = formats::test(&iso, None, &CancelToken::new(), &Progress::new()).unwrap();
    assert!(t.iter().any(|(n, _)| n.ends_with("b.bin")) && t.iter().all(|(_, r)| r.is_ok()), "{:?}", t.iter().map(|(n, r)| (n, r.is_ok())).collect::<Vec<_>>());
    // a truncated image must not test clean
    let bytes = fs::read(&iso).unwrap();
    let cut = w.path().join("cut.iso");
    fs::write(&cut, &bytes[..bytes.len() / 3]).unwrap();
    match formats::test(&cut, None, &CancelToken::new(), &Progress::new()) {
        Ok(r) => assert!(r.iter().any(|x| x.1.is_err()), "truncated ISO tested clean"),
        Err(e) => assert!(matches!(e.kind, ErrorKind::BrokenArchive | ErrorKind::UnsupportedFormat | ErrorKind::Io), "{e}"),
    }
    let out = TempDir::new().unwrap();
    extract(&iso, out.path());
    for rel in ["a.txt", "sub/b.bin", "sub/deep/c.txt"] {
        assert_eq!(fs::read(out.path().join(rel)).unwrap(), fs::read(src.join(rel)).unwrap(), "{rel}");
    }
}

#[test]
fn xar_archive() {
    need!("bsdtar");
    let w = TempDir::new().unwrap();
    let src = w.path().join("src");
    make_tree(&src, true);
    let o = Command::new("bsdtar").current_dir(&src).args(["--format", "xar", "-cf", "../t.xar", "a.txt", "sub", "emptydir", "empty.dat", "link.txt"]).output().unwrap();
    if !o.status.success() { eprintln!("SKIP: bsdtar cannot write xar"); return; }
    check_roundtrip(&w.path().join("t.xar"), &src, "xar");
}

#[test]
fn cab_via_gcab() {
    need!("gcab");
    let w = TempDir::new().unwrap();
    let src = w.path().join("src");
    write(&src.join("a.txt"), b"hello legacy world\n");
    write(&src.join("sub/b.bin"), &noise(700_000, 7));
    write(&src.join("sub/deep/c.txt"), "unicode \u{e9}\u{4e2d}\n".as_bytes());
    run(&src, "gcab -c ../t.cab a.txt sub/b.bin sub/deep/c.txt 2>&1 || gcab -c -n ../t.cab a.txt sub/b.bin sub/deep/c.txt");
    let cab = w.path().join("t.cab");
    assert_eq!(detect(&cab).unwrap(), Format::Legacy);
    let out = TempDir::new().unwrap();
    extract(&cab, out.path());
    assert_eq!(fs::read(out.path().join("a.txt")).unwrap(), b"hello legacy world\n");
    assert_eq!(fs::read(out.path().join("sub/b.bin")).unwrap(), fs::read(src.join("sub/b.bin")).unwrap());
    assert_eq!(fs::read(out.path().join("sub/deep/c.txt")).unwrap(), fs::read(src.join("sub/deep/c.txt")).unwrap());
    assert!(formats::test(&cab, None, &CancelToken::new(), &Progress::new()).unwrap().iter().all(|x| x.1.is_ok()));
}

#[test]
fn lzip_and_unix_compress_single_streams() {
    let w = TempDir::new().unwrap();
    let data = noise(500_000, 5).into_iter().map(|b| b % 16).collect::<Vec<u8>>(); // compressible
    if have("lzip") {
        let d = w.path().join("lz");
        write(&d.join("payload.bin"), &data);
        run(&d, "lzip -k payload.bin");
        let lz = d.join("payload.bin.lz");
        assert_eq!(detect(&lz).unwrap(), Format::Legacy);
        let out = TempDir::new().unwrap();
        extract(&lz, out.path());
        assert_eq!(fs::read(out.path().join("payload.bin")).unwrap(), data);
        assert!(formats::test(&lz, None, &CancelToken::new(), &Progress::new()).unwrap().iter().all(|x| x.1.is_ok()));
    } else { eprintln!("SKIP: lzip"); }
    if have("compress") {
        let d = w.path().join("z");
        write(&d.join("payload.bin"), &data);
        run(&d, "compress -k payload.bin || compress -c payload.bin > payload.bin.Z");
        let z = d.join("payload.bin.Z");
        assert_eq!(detect(&z).unwrap(), Format::Legacy);
        let out = TempDir::new().unwrap();
        extract(&z, out.path());
        assert_eq!(fs::read(out.path().join("payload.bin")).unwrap(), data);
    } else { eprintln!("SKIP: compress"); }
}

#[test]
fn tar_lz_through_libarchive() {
    need!("lzip", "tar");
    let w = TempDir::new().unwrap();
    let src = w.path().join("src");
    make_tree(&src, true);
    run(&src, "tar cf - . | lzip > ../t.tar.lz");
    let a = w.path().join("t.tar.lz");
    assert_eq!(detect(&a).unwrap(), Format::Legacy);
    let out = TempDir::new().unwrap();
    extract(&a, out.path());
    assert_eq!(fs::read(out.path().join("sub/b.bin")).unwrap(), fs::read(src.join("sub/b.bin")).unwrap());
}

// ------------------------------------------------------------------ hand-built containers
fn newc(entries: &[(&str, u32, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let pad = |o: &mut Vec<u8>| while o.len() % 4 != 0 { o.push(0) };
    let mut ino = 1u32;
    let mut emit = |out: &mut Vec<u8>, name: &str, mode: u32, data: &[u8], ino: u32| {
        let nm = format!("{name}\0");
        out.extend_from_slice(format!("070701{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}", ino, mode, 0, 0, 1, 1_600_000_000u32, data.len(), 0, 0, 0, 0, nm.len(), 0).as_bytes());
        out.extend_from_slice(nm.as_bytes());
        pad(out);
        out.extend_from_slice(data);
        pad(out);
    };
    for (n, m, d) in entries { emit(&mut out, n, *m, d, ino); ino += 1; }
    emit(&mut out, "TRAILER!!!", 0, b"", 0);
    out
}

fn crc16(d: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in d { c ^= b as u16; for _ in 0..8 { c = if c & 1 != 0 { (c >> 1) ^ 0xA001 } else { c >> 1 }; } }
    c
}
/// LHA level-0 header, `-lh0-` (stored).
fn lha(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, data) in entries {
        let mut h = Vec::new();
        h.extend_from_slice(b"-lh0-");
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        let date: u16 = ((2020 - 1980) << 9) | (1 << 5) | 1;
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&date.to_le_bytes());
        h.push(0x20);
        h.push(0);
        h.push(name.len() as u8);
        h.extend_from_slice(name.as_bytes());
        h.extend_from_slice(&crc16(data).to_le_bytes());
        out.push(h.len() as u8);
        out.push(h.iter().fold(0u8, |a, b| a.wrapping_add(*b)));
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
    }
    out.push(0);
    out
}

#[test]
fn lha_lzh_stored() {
    let w = TempDir::new().unwrap();
    let big = noise(300_000, 21);
    let a = w.path().join("t.lzh");
    fs::write(&a, lha(&[("readme.txt", b"lha says hi\n"), ("dir\\big.bin", &big)])).unwrap();
    assert_eq!(detect(&a).unwrap(), Format::Legacy);
    let items = formats::list(&a, None, None, &CancelToken::new()).unwrap();
    assert_eq!(items.len(), 2, "{:?}", items.iter().map(|i| &i.name).collect::<Vec<_>>());
    assert!(items.iter().all(|i| i.mtime > 1_500_000_000));
    let out = TempDir::new().unwrap();
    extract(&a, out.path());
    assert_eq!(fs::read(out.path().join("readme.txt")).unwrap(), b"lha says hi\n");
    assert_eq!(fs::read(out.path().join("dir/big.bin")).unwrap(), big);
    // third-party decoder agrees
    if have("lha") {
        let o2 = TempDir::new().unwrap();
        run(o2.path(), &format!("lha xq {}", a.display()));
        assert_eq!(fs::read(o2.path().join("readme.txt")).unwrap(), b"lha says hi\n");
    }
    // corrupt a payload byte: the stored CRC must catch it
    let mut bad = fs::read(&a).unwrap();
    let last = bad.len() - 10;
    bad[last] ^= 0xff;
    let b = w.path().join("bad.lzh");
    fs::write(&b, bad).unwrap();
    let t = formats::test(&b, None, &CancelToken::new(), &Progress::new()).unwrap();
    assert!(t.iter().any(|x| x.1.is_err()), "corruption undetected: {:?}", t.iter().map(|x| (&x.0, x.1.is_ok())).collect::<Vec<_>>());
}

#[test]
fn rpm_cpio_payload() {
    let w = TempDir::new().unwrap();
    let payload = newc(&[("./usr/bin/hello", 0o100755, b"#!/bin/sh\necho hi\n"), ("./usr/share/doc/README", 0o100644, &noise(100_000, 2))]);
    let mut rpm = vec![0xed, 0xab, 0xee, 0xdb, 3, 0, 0, 0, 0, 1];
    rpm.resize(96, 0); // lead
    for _ in 0..2 { rpm.extend_from_slice(&[0x8e, 0xad, 0xe8, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]); } // empty signature + main headers
    rpm.extend_from_slice(&payload);
    let p = w.path().join("hello-1.0.rpm");
    fs::write(&p, rpm).unwrap();
    assert_eq!(detect(&p).unwrap(), Format::Legacy);
    let out = TempDir::new().unwrap();
    extract(&p, out.path());
    assert_eq!(fs::read(out.path().join("usr/bin/hello")).unwrap(), b"#!/bin/sh\necho hi\n");
    assert_eq!(fs::read(out.path().join("usr/share/doc/README")).unwrap().len(), 100_000);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(out.path().join("usr/bin/hello")).unwrap().permissions().mode() & 0o111, 0o111);
    }
}

// ------------------------------------------------------------------ safety
fn tree_has_name(root: &Path, name: &str) -> bool {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap().flatten() {
            if e.file_name() == name { return true; }
            if e.file_type().unwrap().is_dir() { stack.push(e.path()); }
        }
    }
    false
}

#[test]
fn hostile_cpio_never_escapes_destination() {
    let w = TempDir::new().unwrap();
    let sandbox = w.path().join("sandbox");
    let dest = sandbox.join("dest");
    fs::create_dir_all(&dest).unwrap();
    let abs = w.path().join("abs_evil");
    let abs_s = abs.to_string_lossy().into_owned();
    let data = newc(&[
        ("good.txt", 0o100644, b"ok"),
        ("../evil", 0o100644, b"pwn"),
        ("a/../../evil2", 0o100644, b"pwn"),
        (&abs_s, 0o100644, b"pwn"),
        ("up", 0o120777, b"../.."),
        ("abslink", 0o120777, b"/etc"),
        ("okdir", 0o040755, b""),
        ("okdir/rel", 0o120777, b"../good.txt"),
        ("dev_null", 0o020666, b""),
        ("pipe", 0o010644, b""),
    ]);
    let a = dest.parent().unwrap().join("hostile.cpio");
    fs::write(&a, data).unwrap();
    assert_eq!(detect(&a).unwrap(), Format::Legacy);
    let rep = formats::extract(&a, &opts(&dest), None, &CancelToken::new(), &Progress::new()).unwrap();
    assert!(!sandbox.join("evil").exists() && !w.path().join("evil").exists() && !w.path().join("evil2").exists() && !sandbox.join("evil2").exists());
    assert!(!abs.exists(), "absolute path escaped");
    assert!(!tree_has_name(w.path(), "evil") && !tree_has_name(w.path(), "evil2"));
    assert_eq!(fs::read(dest.join("good.txt")).unwrap(), b"ok");
    for n in ["../evil", "a/../../evil2", &abs_s] { assert!(rep.unsafe_refused.iter().any(|r| r == n), "{n} not refused: {:?}", rep.unsafe_refused); }
    assert!(rep.unsafe_refused.iter().any(|r| r.starts_with("up ->")));
    assert!(rep.unsafe_refused.iter().any(|r| r.starts_with("abslink ->")));
    assert!(fs::symlink_metadata(dest.join("up")).is_err() && fs::symlink_metadata(dest.join("abslink")).is_err());
    assert_eq!(fs::read_link(dest.join("okdir/rel")).unwrap(), Path::new("../good.txt"));
    assert!(fs::symlink_metadata(dest.join("dev_null")).is_err(), "device node created");
    assert!(fs::symlink_metadata(dest.join("pipe")).is_err(), "fifo created");
    assert!(rep.skipped >= 2);
}

#[test]
fn symlinked_ancestor_is_not_followed() {
    let w = TempDir::new().unwrap();
    let dest = w.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    // `d` -> `.` is an in-tree link, `d/inner` would be written through it: refused rather than followed
    let a = w.path().join("t.cpio");
    fs::write(&a, newc(&[("d", 0o120777, b"."), ("d/inner.txt", 0o100644, b"x"), ("plain.txt", 0o100644, b"y")])).unwrap();
    let rep = formats::extract(&a, &opts(&dest), None, &CancelToken::new(), &Progress::new()).unwrap();
    assert!(rep.unsafe_refused.iter().any(|r| r == "d/inner.txt"), "{rep:?}");
    assert!(dest.join("plain.txt").exists());
}

#[test]
fn hostile_lha_names_refused() {
    let w = TempDir::new().unwrap();
    let dest = w.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    let a = w.path().join("evil.lzh");
    fs::write(&a, lha(&[("../evil", b"x"), ("fine.txt", b"y")])).unwrap();
    let rep = formats::extract(&a, &opts(&dest), None, &CancelToken::new(), &Progress::new()).unwrap();
    assert!(!w.path().join("evil").exists());
    assert!(dest.join("fine.txt").exists());
    assert!(!rep.unsafe_refused.is_empty(), "{rep:?}");
}

#[test]
fn hardlinks_in_cpio() {
    let w = TempDir::new().unwrap();
    let dest = w.path().join("dest");
    // two names, same inode: first carries no data, last carries it (newc convention)
    let mut v = Vec::new();
    let mut emit = |name: &str, ino: u32, nlink: u32, data: &[u8]| {
        let nm = format!("{name}\0");
        v.extend_from_slice(format!("070701{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}", ino, 0o100644, 0, 0, nlink, 1_600_000_000u32, data.len(), 0, 0, 0, 0, nm.len(), 0).as_bytes());
        v.extend_from_slice(nm.as_bytes());
        while v.len() % 4 != 0 { v.push(0) }
        v.extend_from_slice(data);
        while v.len() % 4 != 0 { v.push(0) }
    };
    emit("first", 9, 2, b"");
    emit("second", 9, 2, b"shared body");
    emit("TRAILER!!!", 0, 1, b"");
    let a = w.path().join("h.cpio");
    fs::write(&a, v).unwrap();
    let rep = formats::extract(&a, &opts(&dest), None, &CancelToken::new(), &Progress::new()).unwrap();
    assert!(rep.failed.is_empty(), "{rep:?}");
    assert_eq!(fs::read(dest.join("second")).unwrap(), b"shared body");
    assert_eq!(fs::read(dest.join("first")).unwrap(), b"shared body");
}

#[test]
fn cancel_and_errors() {
    need!("cpio");
    let w = TempDir::new().unwrap();
    let src = w.path().join("src");
    make_tree(&src, false);
    run(&src, "find . -mindepth 1 | cpio -o -H newc --quiet > ../t.cpio");
    let a = w.path().join("t.cpio");
    let c = CancelToken::new();
    c.cancel();
    let out = TempDir::new().unwrap();
    let e = formats::extract(&a, &opts(out.path()), None, &c, &Progress::new()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Cancelled);
    // truncated archive: test() must report the damage, not claim success
    let bytes = fs::read(&a).unwrap();
    let t = w.path().join("trunc.cpio");
    fs::write(&t, &bytes[..bytes.len() / 2]).unwrap();
    match formats::test(&t, None, &CancelToken::new(), &Progress::new()) {
        Ok(r) => assert!(r.iter().any(|x| x.1.is_err()), "truncation undetected"),
        Err(_) => {}
    }
    // random bytes are not a legacy archive
    let junk = w.path().join("junk.bin");
    fs::write(&junk, noise(5000, 99)).unwrap();
    assert_eq!(detect(&junk).unwrap(), Format::Unknown);
    assert_eq!(formats::list(&junk, None, None, &CancelToken::new()).unwrap_err().kind, ErrorKind::UnsupportedFormat);
    // existing formats still win detection
    let tgz = w.path().join("x.tgz");
    run(&src, &format!("tar czf {} .", tgz.display()));
    assert!(matches!(detect(&tgz).unwrap(), Format::Tar(_)));
}

// ------------------------------------------------------------------ UUE / XXE / Base64
fn encode_uu(data: &[u8], name: &str, mode: u32, alpha: &[u8; 64]) -> Vec<u8> {
    let mut o = format!("begin {mode:o} {name}\n").into_bytes();
    for chunk in data.chunks(45) {
        o.push(alpha[chunk.len()]);
        for g in chunk.chunks(3) {
            let b = [g[0], *g.get(1).unwrap_or(&0), *g.get(2).unwrap_or(&0)];
            o.extend_from_slice(&[alpha[(b[0] >> 2) as usize], alpha[(((b[0] & 3) << 4) | (b[1] >> 4)) as usize], alpha[(((b[1] & 15) << 2) | (b[2] >> 6)) as usize], alpha[(b[2] & 63) as usize]]);
        }
        o.push(b'\n');
    }
    o.push(alpha[0]);
    o.extend_from_slice(b"\nend\n");
    o
}
fn uu_alpha() -> [u8; 64] { let mut a = [0u8; 64]; for i in 0..64 { a[i] = if i == 0 { b'`' } else { 32 + i as u8 }; } a }
fn xx_alpha() -> [u8; 64] { let mut a = [0u8; 64]; a.copy_from_slice(b"+-0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"); a }

#[test]
fn uue_xxe_base64_decode() {
    let w = TempDir::new().unwrap();
    let data = noise(10_007, 17);
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("p.uue", encode_uu(&data, "payload.bin", 0o640, &uu_alpha()), "payload.bin"),
        ("p.xxe", encode_uu(&data, "payload.bin", 0o640, &xx_alpha()), "payload.bin"),
        ("p.b64", {
            let a = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut o = b"begin-base64 644 payload.bin\n".to_vec();
            for c in data.chunks(45) {
                for g in c.chunks(3) {
                    let b = [g[0], *g.get(1).unwrap_or(&0), *g.get(2).unwrap_or(&0)];
                    let q = [a[(b[0] >> 2) as usize], a[(((b[0] & 3) << 4) | (b[1] >> 4)) as usize], a[(((b[1] & 15) << 2) | (b[2] >> 6)) as usize], a[(b[2] & 63) as usize]];
                    let keep = g.len() + 1;
                    for (i, ch) in q.iter().enumerate() { o.push(if i < keep { *ch } else { b'=' }); }
                }
                o.push(b'\n');
            }
            o.extend_from_slice(b"====\n");
            o
        }, "payload.bin"),
    ];
    for (file, enc, name) in cases {
        let p = w.path().join(file);
        fs::write(&p, enc).unwrap();
        assert_eq!(detect(&p).unwrap(), Format::Legacy, "{file}");
        let items = formats::list(&p, None, None, &CancelToken::new()).unwrap();
        assert_eq!((items[0].name.as_str(), items[0].size), (name, data.len() as u64), "{file}");
        assert!(formats::test(&p, None, &CancelToken::new(), &Progress::new()).unwrap()[0].1.is_ok());
        let out = TempDir::new().unwrap();
        extract(&p, out.path());
        assert_eq!(fs::read(out.path().join(name)).unwrap(), data, "{file}");
    }
    // truncated uuencode (no `end`) is reported broken, not silently accepted
    let mut t = encode_uu(&data, "x.bin", 0o644, &uu_alpha());
    t.truncate(t.len() / 2);
    let p = w.path().join("trunc.uue");
    fs::write(&p, t).unwrap();
    let r = formats::test(&p, None, &CancelToken::new(), &Progress::new()).unwrap();
    assert!(r[0].1.is_err());
    // hostile embedded name is refused
    let p = w.path().join("evil.uue");
    fs::write(&p, encode_uu(b"x", "../../evil", 0o644, &uu_alpha())).unwrap();
    let dest = w.path().join("dest");
    let rep = formats::extract(&p, &opts(&dest), None, &CancelToken::new(), &Progress::new()).unwrap();
    assert_eq!(rep.extracted, 0);
    assert!(!tree_has_name(w.path(), "evil"));
    let _: PathBuf = dest;
}

// ------------------------------------------------------------------ real-world LHA (compressed methods)
fn fixtures() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lha") }

#[test]
fn lha_real_world_lh5_lh7_and_sfx() {
    for name in ["explzh_h0_lh5.lzh", "explzh_h1_lh5.lzh", "explzh_h2_lh5.lzh", "explzh_h2_lh7.lzh", "lh2_222_lh5.lzh", "unix114i_h0_lh5.lzh", "lha213_lh5_long.lzh", "lha255e_subdir.lzh", "lha255e_sfx.exe"] {
        let a = fixtures().join(name);
        assert_eq!(detect(&a).unwrap(), Format::Legacy, "{name}");
        let t = formats::test(&a, None, &CancelToken::new(), &Progress::new()).unwrap();
        assert!(!t.is_empty() && t.iter().all(|x| x.1.is_ok()), "{name}: {:?}", t.iter().map(|x| (&x.0, x.1.is_ok())).collect::<Vec<_>>());
        let out = TempDir::new().unwrap();
        let rep = extract(&a, out.path());
        assert!(rep.extracted >= 1 && rep.failed.is_empty() && rep.unsafe_refused.is_empty(), "{name}: {rep:?}");
        if have("lha") {
            // independent decoder (Lhasa) must produce identical bytes
            let o2 = TempDir::new().unwrap();
            run(o2.path(), &format!("lha xq {}", a.display()));
            assert_tree_eq(o2.path(), out.path());
        }
    }
    // content spot-checks that do not rely on any external tool
    let out = TempDir::new().unwrap();
    extract(&fixtures().join("lha213_lh5_long.lzh"), out.path());
    assert_eq!(fs::metadata(out.path().join("LONG.TXT")).unwrap().len(), 1_241_658);
    let out = TempDir::new().unwrap();
    extract(&fixtures().join("explzh_h2_lh5.lzh"), out.path());
    let gpl = fs::read_to_string(out.path().join("gpl-2")).unwrap();
    assert_eq!(gpl.len(), 18092);
    assert!(gpl.contains("GNU GENERAL PUBLIC LICENSE"));
}

#[test]
fn lha_compressed_corruption_is_detected() {
    let w = TempDir::new().unwrap();
    let mut b = fs::read(fixtures().join("lha213_lh5_long.lzh")).unwrap();
    let mid = b.len() / 2;
    for i in 0..8 { b[mid + i] ^= 0x5a; }
    let p = w.path().join("bad.lzh");
    fs::write(&p, b).unwrap();
    match formats::test(&p, None, &CancelToken::new(), &Progress::new()) {
        Ok(r) => assert!(r.iter().any(|x| x.1.is_err()), "corrupt lh5 stream tested clean"),
        Err(_) => {}
    }
}
