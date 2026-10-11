//! Round-trip, detection, safety and cancellation tests for the non-ZIP formats
//! (TAR family, 7z, single-stream codecs) through the library API.
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use fz_core::enumerate::{enumerate, EnumOptions};
use fz_core::formats::codecs::Codec;
use fz_core::formats::sevenz::{create_7z, SevenOptions, Solid, SzMethod};
use fz_core::formats::tar::{create_single_stream, create_tar, TarOptions};
use fz_core::formats::{self, detect, Format};
use fz_core::zip::{create_zip, ExtractOptions, Level, ZipOptions};
use fz_core::{CancelToken, ErrorKind, Progress};
use tempfile::TempDir;

const PW: &str = "correct horse battery";

// ------------------------------------------------------------------ helpers

fn cancel() -> CancelToken {
    CancelToken::new()
}

fn progress() -> Progress {
    Progress::new()
}

fn write(p: &Path, data: &[u8]) {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, data).unwrap();
}

/// Compressible text of exactly `n` bytes.
fn text(n: usize) -> Vec<u8> {
    let line = b"The quick brown fox jumps over the lazy dog 0123456789\n";
    line.iter().cycle().take(n).copied().collect()
}

/// Incompressible bytes (xorshift), deterministic per seed.
fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// `root/proj` with CJK, emoji and space names, an empty dir, an empty file,
/// a symlink and an executable file.
fn build_tree(root: &Path) -> PathBuf {
    let p = root.join("proj");
    write(&p.join("中文 文件.txt"), &text(2000));
    write(&p.join("emoji 😀.bin"), &noise(5000, 7));
    write(&p.join("sub dir/empty.txt"), b"");
    fs::create_dir_all(p.join("空 目录")).unwrap();
    write(&p.join("run.sh"), b"#!/bin/sh\necho hi\n");
    fs::set_permissions(p.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("中文 文件.txt", p.join("link")).unwrap();
    p
}

/// `root/a` and `root/b`: two top-level folders (multi-root archive).
fn build_multi(root: &Path) -> (PathBuf, PathBuf) {
    let a = root.join("a");
    let b = root.join("b");
    write(&a.join("x 一.txt"), &text(777));
    write(&b.join("y 😀.txt"), &text(333));
    (a, b)
}

fn entries_of(inputs: &[PathBuf]) -> Vec<fz_core::enumerate::EntryDesc> {
    enumerate(inputs, &EnumOptions::default(), &cancel()).unwrap()
}

/// These historical 7z fixture tests compare dereferenced bytes. Make that choice explicit:
/// the production writer must no longer follow stored symlinks silently.
fn dereferenced_entries(entries: &[fz_core::enumerate::EntryDesc]) -> Vec<fz_core::enumerate::EntryDesc> {
    entries.iter().cloned().map(|mut entry| {
        if entry.kind == fz_core::enumerate::EntryKind::Symlink {
            entry.kind = fz_core::enumerate::EntryKind::File;
            entry.size = fs::metadata(&entry.abs).unwrap().len();
            entry.link_target = None;
        }
        entry
    }).collect()
}

#[test]
fn sevenz_stored_symlink_requires_explicit_dereference() {
    let t = TempDir::new().unwrap();
    let src = build_tree(t.path());
    let entries = entries_of(&[src]);
    let out = t.path().join("existing.7z");
    fs::write(&out, b"original").unwrap();
    let err = create_7z(&out, &entries, &SevenOptions::default(), &cancel(), &progress()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnsupportedFormat);
    assert_eq!(fs::read(&out).unwrap(), b"original");
}

fn collect_rel(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    for e in fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let md = fs::symlink_metadata(e.path()).unwrap();
        out.push(rel.clone());
        if md.is_dir() {
            collect_rel(&e.path(), &rel, out);
        }
    }
}

/// Asserts that `dst` mirrors `src`: same names, same bytes, empty dirs kept, and
/// (when `check_links`) symlinks kept as links. The exec bit of `run.sh` is always checked.
fn assert_same_tree(src: &Path, dst: &Path, check_links: bool, check_exec: bool) {
    let mut names = Vec::new();
    collect_rel(src, "", &mut names);
    assert!(!names.is_empty());
    for rel in &names {
        let s = src.join(rel);
        let d = dst.join(rel);
        let smd = fs::symlink_metadata(&s).unwrap();
        if smd.file_type().is_symlink() && check_links {
            let dmd = fs::symlink_metadata(&d).unwrap_or_else(|_| panic!("missing symlink {rel}"));
            assert!(dmd.file_type().is_symlink(), "{rel}: symlink not kept as link");
            assert_eq!(fs::read_link(&s).unwrap(), fs::read_link(&d).unwrap(), "{rel}: link target");
        } else if smd.is_dir() {
            assert!(d.is_dir(), "{rel}: directory missing");
        } else {
            let got = fs::read(&d).unwrap_or_else(|e| panic!("{rel}: {e}"));
            assert_eq!(fs::read(&s).unwrap(), got, "{rel}: content differs");
        }
        if check_exec && rel.ends_with("run.sh") {
            let mode = fs::metadata(&d).unwrap().permissions().mode();
            assert!(mode & 0o111 != 0, "{rel}: exec bit lost (mode {mode:o})");
        }
    }
}

/// Runs an external tool as `bin args... file`. None when the tool is not installed.
fn tool_ok(bin: &str, args: &[&str], file: &Path) -> Option<bool> {
    match Command::new(bin).args(args).arg(file).output() {
        Ok(o) => Some(o.status.success()),
        Err(_) => None,
    }
}

fn tar_name(codec: Codec) -> String {
    if codec == Codec::None { "bundle.tar".into() } else { format!("bundle.tar.{}", codec.ext()) }
}

fn sz_opts(method: SzMethod, solid: Solid, level: Level) -> SevenOptions {
    SevenOptions { method, solid, level, ..Default::default() }
}

// ------------------------------------------------------- 1. TAR family

#[test]
fn tar_family_round_trip_all_codecs() {
    let codecs = [Codec::None, Codec::Gzip, Codec::Bzip2, Codec::Xz, Codec::Zstd, Codec::Lz4, Codec::Brotli, Codec::Lzma];
    let t = TempDir::new().unwrap();
    let src = build_tree(t.path());
    let entries = entries_of(&[src.clone()]);
    for codec in codecs {
        let dir = t.path().join(format!("tar-{codec:?}"));
        fs::create_dir_all(&dir).unwrap();
        let out = dir.join(tar_name(codec));
        let o = TarOptions { codec, ..Default::default() };
        create_tar(&out, &entries, &o, &cancel(), &progress()).unwrap();
        assert!(!dir.join(format!("{}.part", tar_name(codec))).exists());

        assert_eq!(detect(&out).unwrap(), Format::Tar(codec), "{codec:?}: detect");

        let items = formats::list(&out, None, None, &cancel()).unwrap();
        let names: Vec<String> = items.iter().map(|i| i.name.trim_end_matches('/').to_string()).collect();
        assert!(names.contains(&"proj/中文 文件.txt".to_string()), "{codec:?}: {names:?}");
        assert!(names.contains(&"proj/空 目录".to_string()), "{codec:?}: empty dir not listed");

        let dest = dir.join("out");
        let o = ExtractOptions { smart: false, ..ExtractOptions::new(&dest) };
        let rep = formats::extract(&out, &o, None, &cancel(), &progress()).unwrap();
        assert!(rep.unsafe_refused.is_empty(), "{codec:?}: {:?}", rep.unsafe_refused);
        assert_same_tree(&src, &dest.join("proj"), true, true);

        // external cross-checks
        let tar_ok = match codec { Codec::Lz4 | Codec::Brotli | Codec::Lzma => None, _ => tool_ok("tar", &["-tf"], &out) };
        let ext_ok = match codec {
            Codec::None => None,
            Codec::Gzip => tool_ok("gzip", &["-t"], &out),
            Codec::Bzip2 => tool_ok("bzip2", &["-t"], &out),
            Codec::Xz => tool_ok("xz", &["-t"], &out),

            _ => None,
        };
        if let Some(ok) = tar_ok {
            assert!(ok, "{codec:?}: system tar rejects archive");
        } else {
            eprintln!("skip: {codec:?} external tar check unavailable (unsupported codec or missing tool)");
        }
        if let Some(ok) = ext_ok {
            assert!(ok, "{codec:?}: codec tool rejects archive");
        }
    }
}

// ------------------------------------------------------- 2. 7z

#[test]
fn sevenz_round_trip_methods_solid_and_levels() {
    let t = TempDir::new().unwrap();
    let src = build_tree(t.path());
    let entries = entries_of(&[src.clone()]);
    let methods = [SzMethod::Lzma2, SzMethod::Lzma, SzMethod::Ppmd, SzMethod::Bzip2, SzMethod::Copy, SzMethod::Zstd, SzMethod::Deflate];
    let solids = [Solid::None, Solid::Single, Solid::Bytes(4096)];
    let mut n = 0;
    for method in methods {
        for solid in solids {
            let dir = t.path().join(format!("7z-{method:?}-{solid:?}").replace(['(', ')', ' '], "_"));
            fs::create_dir_all(&dir).unwrap();
            let out = dir.join("a.7z");
            create_7z(&out, &dereferenced_entries(&entries), &sz_opts(method, solid, Level::Normal), &cancel(), &progress()).unwrap();
            assert_eq!(detect(&out).unwrap(), Format::SevenZ);
            let dest = dir.join("out");
            let o = ExtractOptions { smart: false, ..ExtractOptions::new(&dest) };
            formats::extract(&out, &o, None, &cancel(), &progress()).unwrap();
            assert_same_tree(&src, &dest.join("proj"), false, false);
            n += 1;
        }
    }
    // levels with LZMA2
    for level in [Level::Fast, Level::Maximum, Level::Store] {
        let dir = t.path().join(format!("7z-level-{level:?}"));
        fs::create_dir_all(&dir).unwrap();
        let out = dir.join("l.7z");
        create_7z(&out, &dereferenced_entries(&entries), &sz_opts(SzMethod::Lzma2, Solid::Bytes(64 << 20), level), &cancel(), &progress()).unwrap();
        let dest = dir.join("out");
        let o = ExtractOptions { smart: false, ..ExtractOptions::new(&dest) };
        formats::extract(&out, &o, None, &cancel(), &progress()).unwrap();
        assert_same_tree(&src, &dest.join("proj"), false, false);
        n += 1;
    }
    assert_eq!(n, methods.len() * solids.len() + 3);
}

#[test]
fn sevenz_system_7z_test_accepts_archives() {
    let t = TempDir::new().unwrap();
    let src = build_tree(t.path());
    let entries = entries_of(&[src]);
    for method in [SzMethod::Lzma2, SzMethod::Ppmd, SzMethod::Bzip2, SzMethod::Deflate] {
        let out = t.path().join(format!("chk-{method:?}.7z"));
        create_7z(&out, &dereferenced_entries(&entries), &sz_opts(method, Solid::Single, Level::Normal), &cancel(), &progress()).unwrap();
        match tool_ok("7z", &["t"], &out) {
            Some(ok) => assert!(ok, "{method:?}: 7z t rejects archive"),
            None => {
                eprintln!("skip: 7z not installed");
                return;
            }
        }
    }
}

#[test]
fn sevenz_password_round_trip_and_errors() {
    let t = TempDir::new().unwrap();
    let src = build_tree(t.path());
    let entries = entries_of(&[src.clone()]);

    // plain header: names visible, data encrypted
    let out = t.path().join("pw.7z");
    let o = SevenOptions { password: Some(PW.into()), encrypt_names: false, ..Default::default() };
    create_7z(&out, &dereferenced_entries(&entries), &o, &cancel(), &progress()).unwrap();
    let dest = t.path().join("pw-out");
    let eo = ExtractOptions { smart: false, password: Some(PW.into()), ..ExtractOptions::new(&dest) };
    formats::extract(&out, &eo, None, &cancel(), &progress()).unwrap();
    assert_same_tree(&src, &dest.join("proj"), false, false);
    assert!(formats::extract(&out, &ExtractOptions { smart: false, ..ExtractOptions::new(t.path().join("nopw")) }, None, &cancel(), &progress()).is_err());

    // encrypted header
    let out = t.path().join("pwn.7z");
    let o = SevenOptions { password: Some(PW.into()), encrypt_names: true, ..Default::default() };
    create_7z(&out, &dereferenced_entries(&entries), &o, &cancel(), &progress()).unwrap();
    let e = formats::list(&out, None, None, &cancel()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::PasswordRequired, "list without password: {e}");
    let e = formats::list(&out, Some("definitely wrong"), None, &cancel()).unwrap_err();
    assert!(matches!(e.kind, ErrorKind::WrongPassword | ErrorKind::PasswordRequired), "wrong pw: {e}");
    let items = formats::list(&out, Some(PW), None, &cancel()).unwrap();
    assert!(items.iter().any(|i| i.name.ends_with("中文 文件.txt")));
    let wrong = ExtractOptions { smart: false, password: Some("definitely wrong".into()), ..ExtractOptions::new(t.path().join("wrong")) };
    assert!(formats::extract(&out, &wrong, None, &cancel(), &progress()).is_err());
    let dest = t.path().join("pwn-out");
    let eo = ExtractOptions { smart: false, password: Some(PW.into()), ..ExtractOptions::new(&dest) };
    formats::extract(&out, &eo, None, &cancel(), &progress()).unwrap();
    assert_same_tree(&src, &dest.join("proj"), false, false);
}

#[test]
fn sevenz_split_volumes_join_and_extract() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("src");
    let data = noise(500 * 1024, 3);
    write(&src.join("big.bin"), &data);
    let entries = entries_of(&[src.join("big.bin")]);
    let out = t.path().join("big.7z");
    let o = SevenOptions { split_size: Some(100 * 1024), ..Default::default() };
    create_7z(&out, &dereferenced_entries(&entries), &o, &cancel(), &progress()).unwrap();
    for i in 1..=5 {
        assert!(t.path().join(format!("big.7z.{i:03}")).exists(), "volume {i:03} missing");
    }
    assert!(!out.exists(), "unsplit archive should not be left behind");

    let first = t.path().join("big.7z.001");
    let items = formats::list(&first, None, None, &cancel()).unwrap();
    assert!(items.iter().any(|i| i.name == "big.bin" && i.size == data.len() as u64));
    let dest = t.path().join("dest");
    let o = ExtractOptions { smart: false, ..ExtractOptions::new(&dest) };
    formats::extract(&first, &o, None, &cancel(), &progress()).unwrap();
    assert!(fs::read(dest.join("big.bin")).unwrap() == data, "split round trip differs");
    if let Some(ok) = tool_ok("7z", &["t"], &first) {
        assert!(ok, "7z t rejects split set");
    }
}

#[test]

fn sevenz_extract_keeps_exec_bit() {
    let t = TempDir::new().unwrap();
    let src = build_tree(t.path());
    let entries = entries_of(&[src.clone()]);
    let out = t.path().join("exec.7z");
    create_7z(&out, &dereferenced_entries(&entries), &sz_opts(SzMethod::Lzma2, Solid::Bytes(64 << 20), Level::Normal), &cancel(), &progress()).unwrap();
    let dest = t.path().join("out");
    let o = ExtractOptions { smart: false, ..ExtractOptions::new(&dest) };
    formats::extract(&out, &o, None, &cancel(), &progress()).unwrap();
    assert_same_tree(&src, &dest.join("proj"), false, true);
}

// ------------------------------------------------------- 3. single-stream

#[test]
fn single_stream_round_trip_all_codecs() {
    let t = TempDir::new().unwrap();
    let input = t.path().join("data.txt");
    let mut body = text(100_000);
    body.extend(noise(10_000, 11));
    write(&input, &body);
    for codec in [Codec::Gzip, Codec::Xz, Codec::Zstd, Codec::Bzip2, Codec::Lz4, Codec::Brotli] {
        let dir = t.path().join(format!("s-{codec:?}"));
        fs::create_dir_all(&dir).unwrap();
        let out = dir.join(format!("data.txt.{}", codec.ext()));
        create_single_stream(&out, &input, codec, Level::Normal, 0, &cancel(), &progress()).unwrap();
        assert_eq!(detect(&out).unwrap(), Format::Stream(codec), "{codec:?}: detect");
        formats::test(&out, None, &cancel(), &progress()).unwrap().iter().for_each(|(_, r)| assert!(r.is_ok(), "{codec:?}: test failed"));
        let dest = dir.join("out");
        let o = ExtractOptions::new(&dest);
        formats::extract(&out, &o, None, &cancel(), &progress()).unwrap();
        assert_eq!(fs::read(dest.join("data.txt")).unwrap(), body, "{codec:?}: bytes differ");
    }
}

// ------------------------------------------------------- 4. detection by content

#[test]
fn detection_ignores_misleading_extensions() {
    let t = TempDir::new().unwrap();
    let src = build_tree(t.path());
    let entries = entries_of(&[src]);

    let zip = t.path().join("zip-as.dat");
    create_zip(&zip, &entries, &ZipOptions::default(), &cancel(), &progress()).unwrap();
    assert_eq!(detect(&zip).unwrap(), Format::Zip);
    assert!(!formats::list(&zip, None, None, &cancel()).unwrap().is_empty());

    let sz = t.path().join("7z-as.zip");
    create_7z(&sz, &dereferenced_entries(&entries), &SevenOptions::default(), &cancel(), &progress()).unwrap();
    assert_eq!(detect(&sz).unwrap(), Format::SevenZ);
    assert!(!formats::list(&sz, None, None, &cancel()).unwrap().is_empty());

    let tgz = t.path().join("tgz-as.bin");
    create_tar(&tgz, &entries, &TarOptions { codec: Codec::Gzip, ..Default::default() }, &cancel(), &progress()).unwrap();
    assert_eq!(detect(&tgz).unwrap(), Format::Tar(Codec::Gzip));
    let names: Vec<String> = formats::list(&tgz, None, None, &cancel()).unwrap().into_iter().map(|i| i.name).collect();
    assert!(names.iter().any(|n| n.starts_with("proj/")), "{names:?}");
}

// ------------------------------------------------------- 5. safety

/// Raw tar header with the name/linkname bytes written verbatim (bypasses set_path checks).
fn raw_header(name: &str, link: Option<&str>, size: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    let nb = name.as_bytes();
    h.as_old_mut().name[..nb.len()].copy_from_slice(nb);
    if let Some(l) = link {
        let lb = l.as_bytes();
        h.as_old_mut().linkname[..lb.len()].copy_from_slice(lb);
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
    } else {
        h.set_entry_type(tar::EntryType::Regular);
        h.set_size(size);
    }
    h.set_mode(0o644);
    h.set_mtime(0);
    h.set_cksum();
    h
}

#[test]
fn malicious_tar_never_writes_outside_dest() {
    let t = TempDir::new().unwrap();
    let base = t.path().join("base");
    fs::create_dir_all(&base).unwrap();
    let dest = base.join("dest");

    let mut b = tar::Builder::new(Vec::new());
    for (name, data) in [("../evil.txt", &b"pwned"[..]), ("/abs.txt", &b"abs"[..]), ("sub/../../escape.txt", &b"esc"[..]), ("safe/ok.txt", &b"ok"[..])] {
        let h = raw_header(name, None, data.len() as u64);
        b.append(&h, data).unwrap();
    }
    for (name, target) in [("lnk_abs", "/etc/passwd"), ("lnk_up", "../../outside")] {
        let h = raw_header(name, Some(target), 0);
        b.append(&h, &b""[..]).unwrap();
    }
    let bytes = b.into_inner().unwrap();
    let evil = base.join("evil.tar");
    write(&evil, &bytes);

    let names: Vec<String> = formats::list(&evil, None, None, &cancel()).unwrap().into_iter().map(|i| i.name).collect();
    assert!(names.contains(&"../evil.txt".to_string()), "{names:?}");

    let o = ExtractOptions { smart: false, ..ExtractOptions::new(&dest) };
    let rep = formats::extract(&evil, &o, None, &cancel(), &progress()).unwrap();

    assert!(!base.join("evil.txt").exists(), "wrote ../evil.txt outside dest");
    assert!(!base.join("escape.txt").exists(), "wrote escape outside dest");
    assert!(!base.join("outside").exists());
    assert!(!Path::new("/abs.txt").exists(), "wrote absolute path");
    assert!(fs::symlink_metadata(dest.join("lnk_abs")).is_err(), "absolute symlink created");
    assert!(fs::symlink_metadata(dest.join("lnk_up")).is_err(), "escaping symlink created");
    assert_eq!(fs::read(dest.join("safe/ok.txt")).unwrap(), b"ok");

    let refused = rep.unsafe_refused.join("\n");
    assert!(refused.contains("../evil.txt"), "refused: {refused}");
    assert!(refused.contains("/abs.txt"), "refused: {refused}");
    assert!(refused.contains("lnk_abs"), "refused: {refused}");
    assert!(refused.contains("lnk_up"), "refused: {refused}");
}

// ------------------------------------------------------- 6. smart extract

#[test]
fn smart_extract_single_root_not_wrapped_multi_root_wrapped() {
    let t = TempDir::new().unwrap();

    // single root folder -> contents go straight into dest
    let src = build_tree(&t.path().join("one"));
    let entries = entries_of(&[src.clone()]);
    let tgz = t.path().join("single.tar.gz");
    create_tar(&tgz, &entries, &TarOptions { codec: Codec::Gzip, ..Default::default() }, &cancel(), &progress()).unwrap();
    let sz = t.path().join("single.7z");
    create_7z(&sz, &dereferenced_entries(&entries), &SevenOptions::default(), &cancel(), &progress()).unwrap();
    for (archive, label) in [(&tgz, "tar"), (&sz, "7z")] {
        let dest = t.path().join(format!("smart-single-{label}"));
        formats::extract(archive, &ExtractOptions::new(&dest), None, &cancel(), &progress()).unwrap();
        assert!(!dest.join("proj").exists(), "{label}: single root was wrapped");
        assert_same_tree(&src, &dest, true_if_tar(label), true_if_tar(label));
    }

    // two roots -> wrapped in the archive stem
    let (a, b) = build_multi(&t.path().join("multi-src"));
    let entries = entries_of(&[a.clone(), b.clone()]);
    let tgz = t.path().join("multi.tar.gz");
    create_tar(&tgz, &entries, &TarOptions { codec: Codec::Gzip, ..Default::default() }, &cancel(), &progress()).unwrap();
    let sz = t.path().join("multi.7z");
    create_7z(&sz, &dereferenced_entries(&entries), &SevenOptions::default(), &cancel(), &progress()).unwrap();
    for (archive, label) in [(&tgz, "tar"), (&sz, "7z")] {
        let dest = t.path().join(format!("smart-multi-{label}"));
        formats::extract(archive, &ExtractOptions::new(&dest), None, &cancel(), &progress()).unwrap();
        assert_eq!(fs::read(dest.join("multi/a/x 一.txt")).unwrap(), text(777), "{label}");
        assert_eq!(fs::read(dest.join("multi/b/y 😀.txt")).unwrap(), text(333), "{label}");
    }
}

fn true_if_tar(label: &str) -> bool {
    label == "tar"
}

// ------------------------------------------------------- 7. cancellation

#[test]
fn cancelled_creation_returns_cancelled_and_leaves_no_files() {
    let t = TempDir::new().unwrap();
    let src = build_tree(t.path());
    let entries = entries_of(&[src]);
    let c = CancelToken::new();
    c.cancel();

    let dir = t.path().join("cx");
    fs::create_dir_all(&dir).unwrap();
    let out = dir.join("c.tar.gz");
    let e = create_tar(&out, &entries, &TarOptions { codec: Codec::Gzip, ..Default::default() }, &c, &progress()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Cancelled);
    assert!(!out.exists());
    assert!(fs::read_dir(&dir).unwrap().next().is_none(), "leftover files after cancelled tar");

    let out = dir.join("c.7z");
    let e = create_7z(&out, &dereferenced_entries(&entries), &SevenOptions::default(), &c, &progress()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Cancelled);
    assert!(!out.exists());
    assert!(fs::read_dir(&dir).unwrap().next().is_none(), "leftover files after cancelled 7z");
}

// ------------------------------------------------------- 8. corruption

#[test]
fn corrupted_7z_is_reported_by_test() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("src");
    write(&src.join("a.bin"), &noise(60 * 1024, 5));
    write(&src.join("b.txt"), &text(60 * 1024));
    let entries = entries_of(&[src]);
    let out = t.path().join("c.7z");
    create_7z(&out, &dereferenced_entries(&entries), &sz_opts(SzMethod::Lzma2, Solid::None, Level::Normal), &cancel(), &progress()).unwrap();

    let mut bytes = fs::read(&out).unwrap();
    let mid = bytes.len() / 2;
    let end = (mid + 64).min(bytes.len());
    for b in &mut bytes[mid..end] {
        *b ^= 0xFF;
    }
    let bad = t.path().join("corrupt.7z");
    write(&bad, &bytes);

    match formats::test(&bad, None, &cancel(), &progress()) {
        Err(_) => {}
        Ok(v) => assert!(v.iter().any(|(_, r)| r.is_err()), "corruption not reported: {v:?}"),
    }
}

#[test]
fn truncated_tar_gz_is_reported_by_test() {
    let t = TempDir::new().unwrap();
    let src = build_tree(t.path());
    let entries = entries_of(&[src]);
    let out = t.path().join("full.tar.gz");
    create_tar(&out, &entries, &TarOptions { codec: Codec::Gzip, ..Default::default() }, &cancel(), &progress()).unwrap();
    let bytes = fs::read(&out).unwrap();
    let trunc = t.path().join("trunc.tar.gz");
    write(&trunc, &bytes[..bytes.len() / 2]);

    match formats::test(&trunc, None, &cancel(), &progress()) {
        Err(_) => {}
        Ok(v) => assert!(v.iter().any(|(_, r)| r.is_err()), "truncation not reported: {v:?}"),
    }
}
