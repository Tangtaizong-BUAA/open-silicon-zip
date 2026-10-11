//! End-to-end tests of the ZIP engine through the library API:
//! enumerate -> zip::create_zip -> zip::ZipArchive::open -> zip::extract / test_archive.
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use fz_core::enumerate::{enumerate, EntryDesc, EnumOptions};
use fz_core::zip::extract::{ConflictChoice, ConflictFn, ExtractReport};
use fz_core::zip::{
    create_zip, extract, test_archive, Encryption, ExtractOptions, HighSpeed, Level, Method,
    Overwrite, ReadOptions, ZipArchive, ZipEntry, ZipOptions, ZipStats,
};
use fz_core::{CancelToken, ErrorKind, Progress};
use filetime::FileTime;
use tempfile::TempDir;

const MTIME: i64 = 1_600_000_000;
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

fn set_mtime(p: &Path, secs: i64) {
    filetime::set_file_mtime(p, FileTime::from_unix_time(secs, 0)).unwrap();
}

fn enumerate_inputs(inputs: &[PathBuf]) -> Vec<EntryDesc> {
    enumerate(inputs, &EnumOptions::default(), &cancel()).unwrap()
}

fn pack(inputs: &[PathBuf], zip: &Path, opts: &ZipOptions) -> ZipStats {
    let entries = enumerate_inputs(inputs);
    create_zip(zip, &entries, opts, &cancel(), &progress()).unwrap()
}

fn open(zip: &Path) -> ZipArchive {
    ZipArchive::open(zip, ReadOptions::default()).unwrap()
}

fn unpack(zip: &Path, dest: &Path) -> ExtractReport {
    let arc = open(zip);
    let rep = extract(&arc, &ExtractOptions::new(dest), &cancel(), &progress()).unwrap();
    assert!(rep.failed.is_empty(), "failed entries: {:?}", rep.failed);
    rep
}

fn kind_of<T: Debug>(r: fz_core::Result<T>) -> ErrorKind {
    match r {
        Ok(v) => panic!("expected an error, got Ok({v:?})"),
        Err(e) => e.kind,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Node {
    Dir,
    File(Vec<u8>),
    Link(String),
}

/// Relative path -> node for everything under `root` (root itself excluded).
fn snapshot(root: &Path) -> BTreeMap<String, Node> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Node>) {
        for ent in fs::read_dir(dir).unwrap() {
            let p = ent.unwrap().path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            let md = fs::symlink_metadata(&p).unwrap();
            let node = if md.file_type().is_symlink() {
                Node::Link(fs::read_link(&p).unwrap().to_string_lossy().into_owned())
            } else if md.is_dir() {
                walk(root, &p, out);
                Node::Dir
            } else {
                Node::File(fs::read(&p).unwrap())
            };
            out.insert(rel, node);
        }
    }
    let mut m = BTreeMap::new();
    if root.exists() {
        walk(root, root, &mut m);
    }
    m
}

/// Source tree used by several tests: text, noise, empty file, empty dir, exec script, symlink.
fn build_tree(src: &Path) {
    write(&src.join("a.txt"), &text(40_000));
    write(&src.join("sub/b.txt"), b"nested file\n");
    write(&src.join("bin.dat"), &noise(300_000, 7));
    write(&src.join("empty.txt"), b"");
    fs::create_dir_all(src.join("empty_dir")).unwrap();
    write(&src.join("run.sh"), b"#!/bin/sh\necho hi\n");
    fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("a.txt", src.join("link")).unwrap();
    for f in ["a.txt", "sub/b.txt", "bin.dat", "empty.txt", "run.sh"] {
        set_mtime(&src.join(f), MTIME);
    }
}

// ----- minimal raw-zip writer (stored entries, arbitrary names) and parser

fn raw_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut cd: Vec<u8> = Vec::new();
    for (name, data) in entries {
        let name = name.as_bytes();
        let off = out.len() as u32;
        let crc = crc32fast::hash(data);
        let n = data.len() as u32;
        // local header
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0x0021u16.to_le_bytes()); // date 1980-01-01
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(data);
        // central header
        cd.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        cd.extend_from_slice(&((3u16 << 8) | 20).to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0x0021u16.to_le_bytes());
        cd.extend_from_slice(&crc.to_le_bytes());
        cd.extend_from_slice(&n.to_le_bytes());
        cd.extend_from_slice(&n.to_le_bytes());
        cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes()); // extra
        cd.extend_from_slice(&0u16.to_le_bytes()); // comment
        cd.extend_from_slice(&0u16.to_le_bytes()); // disk
        cd.extend_from_slice(&0u16.to_le_bytes()); // internal attr
        cd.extend_from_slice(&0u32.to_le_bytes()); // external attr
        cd.extend_from_slice(&off.to_le_bytes());
        cd.extend_from_slice(name);
    }
    let cd_off = out.len() as u32;
    let cd_len = cd.len() as u32;
    let n = entries.len() as u16;
    out.extend_from_slice(&cd);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

#[derive(Debug)]
struct CdRec {
    ver_made: u16,
    flags: u16,
    method: u16,
    name: Vec<u8>,
    extra: Vec<u8>,
    ext_attr: u32,
}

/// Parses the central directory of a single-disk archive without comment.
fn parse_cd(b: &[u8]) -> Vec<CdRec> {
    let eocd = (0..=b.len() - 22)
        .rev()
        .find(|&i| u32le(b, i) == 0x0605_4b50)
        .expect("EOCD present");
    let n = u16le(b, eocd + 10) as usize;
    let mut p = u32le(b, eocd + 16) as usize;
    let mut v = Vec::new();
    for _ in 0..n {
        assert_eq!(u32le(b, p), 0x0201_4b50, "central directory signature at {p}");
        let nl = u16le(b, p + 28) as usize;
        let el = u16le(b, p + 30) as usize;
        let cl = u16le(b, p + 32) as usize;
        v.push(CdRec {
            ver_made: u16le(b, p + 4),
            flags: u16le(b, p + 8),
            method: u16le(b, p + 10),
            ext_attr: u32le(b, p + 38),
            name: b[p + 46..p + 46 + nl].to_vec(),
            extra: b[p + 46 + nl..p + 46 + nl + el].to_vec(),
        });
        p += 46 + nl + el + cl;
    }
    v
}

fn find_extra(extra: &[u8], id: u16) -> Option<Vec<u8>> {
    let mut e = extra;
    while e.len() >= 4 {
        let i = u16le(e, 0);
        let l = u16le(e, 2) as usize;
        if 4 + l > e.len() {
            break;
        }
        if i == id {
            return Some(e[4..4 + l].to_vec());
        }
        e = &e[4 + l..];
    }
    None
}

fn contains_sig(b: &[u8], sig: u32) -> bool {
    let s = sig.to_le_bytes();
    b.windows(4).any(|w| w == s)
}

// ------------------------------------------------------------------ 1. round trip

#[test]
fn roundtrip_every_method_and_level() {
    for method in [Method::Deflate, Method::Zstd, Method::Bzip2, Method::Store] {
        for level in [Level::Store, Level::Fast, Level::Normal, Level::Maximum] {
            let label = format!("{method:?}/{level:?}");
            let t = TempDir::new().unwrap();
            let src = t.path().join("proj");
            build_tree(&src);
            let zip = t.path().join("out.zip");
            let opts = ZipOptions { method, level, high_speed: HighSpeed::Off, ..Default::default() };
            pack(&[src.clone()], &zip, &opts);

            let arc = open(&zip);
            assert!(
                arc.entries.iter().any(|e| e.is_symlink && e.name == "proj/link"),
                "{label}: symlink not stored as a link"
            );
            assert!(test_archive(&arc, None, &cancel(), &progress()).unwrap().iter().all(|(_, r)| r.is_ok()), "{label}");

            let dest = t.path().join("dest");
            unpack(&zip, &dest);
            assert_eq!(snapshot(&dest), snapshot(&src), "{label}: extracted tree differs from source");

            let want = UNIX_EPOCH + Duration::from_secs(MTIME as u64);
            for f in ["a.txt", "sub/b.txt", "bin.dat", "empty.txt", "run.sh"] {
                let got = fs::metadata(dest.join(f)).unwrap().modified().unwrap();
                let diff = got.duration_since(want).or_else(|_| want.duration_since(got)).unwrap();
                assert!(diff <= Duration::from_secs(2), "{label}: {f} mtime off by {diff:?}");
            }
            let mode = fs::metadata(dest.join("run.sh")).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "{label}: exec bits lost (mode {mode:o})");
            assert!(dest.join("empty_dir").is_dir(), "{label}: empty dir missing");
            assert_eq!(fs::metadata(dest.join("empty.txt")).unwrap().len(), 0, "{label}");
        }
    }
}

// ------------------------------------------------------------------ 2. encryption

fn encrypted_roundtrip(enc: Encryption) {
    let t = TempDir::new().unwrap();
    let src = t.path().join("proj");
    build_tree(&src);
    let zip = t.path().join("enc.zip");
    let opts = ZipOptions { encryption: enc, ..Default::default() };
    pack(&[src.clone()], &zip, &opts);

    let arc = open(&zip);
    assert!(arc.needs_password());

    // missing password
    let no_pw = ExtractOptions::new(t.path().join("nopw"));
    assert_eq!(kind_of(extract(&arc, &no_pw, &cancel(), &progress())), ErrorKind::PasswordRequired);
    let results = test_archive(&arc, None, &cancel(), &progress()).unwrap();
    for (name, r) in results {
        let e = arc.entries.iter().find(|e| e.name == name).unwrap();
        if e.is_dir {
            continue;
        }
        assert_eq!(kind_of(r), ErrorKind::PasswordRequired, "{name}");
    }

    // wrong password
    let wrong = ExtractOptions { password: Some("not the password".into()), ..ExtractOptions::new(t.path().join("bad")) };
    assert_eq!(kind_of(extract(&arc, &wrong, &cancel(), &progress())), ErrorKind::WrongPassword);

    // right password
    let dest = t.path().join("dest");
    let good = ExtractOptions { password: Some(PW.into()), ..ExtractOptions::new(&dest) };
    let rep = extract(&arc, &good, &cancel(), &progress()).unwrap();
    assert!(rep.failed.is_empty(), "{:?}", rep.failed);
    assert_eq!(snapshot(&dest), snapshot(&src));
    let ok = test_archive(&arc, Some(PW), &cancel(), &progress()).unwrap();
    assert!(ok.iter().all(|(_, r)| r.is_ok()), "{ok:?}");
}

#[test]
fn aes256_roundtrip_and_password_errors() {
    encrypted_roundtrip(Encryption::Aes { strength: 3, password: PW.into() });
}

#[test]
fn aes128_roundtrip() {
    encrypted_roundtrip(Encryption::Aes { strength: 1, password: PW.into() });
}

#[test]
fn zipcrypto_roundtrip_and_password_errors() {
    // Note: ZipCrypto's 12-byte header has a 1-byte verifier, so a wrong password
    // is detected with probability 255/256 per entry (random header salt).
    encrypted_roundtrip(Encryption::ZipCrypto(PW.into()));
}

// ------------------------------------------------------------------ 3. high speed

#[test]
fn high_speed_stores_jpg_and_deflates_text() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("media");
    write(&src.join("photo.jpg"), &text(200_000)); // compressible bytes, media name
    write(&src.join("notes.txt"), &text(200_000));

    let find = |arc: &ZipArchive, suffix: &str| -> ZipEntry {
        arc.entries.iter().find(|e| e.name.ends_with(suffix)).cloned().unwrap()
    };

    let on = t.path().join("on.zip");
    pack(&[src.clone()], &on, &ZipOptions::default()); // HighSpeed::On is the default
    let arc = open(&on);
    let jpg = find(&arc, "photo.jpg");
    let txt = find(&arc, "notes.txt");
    assert_eq!(jpg.method, 0, "jpg should be stored in high-speed mode");
    assert_eq!(jpg.csize, jpg.size);
    assert_eq!(txt.method, 8, "txt should be deflated");
    assert!(txt.csize < txt.size / 10);
    unpack(&on, &t.path().join("on_out"));
    assert_eq!(fs::read(t.path().join("on_out/photo.jpg")).unwrap(), text(200_000));

    let off = t.path().join("off.zip");
    let opts = ZipOptions { high_speed: HighSpeed::Off, ..Default::default() };
    pack(&[src.clone()], &off, &opts);
    let arc = open(&off);
    let jpg = find(&arc, "photo.jpg");
    assert_eq!(jpg.method, 8, "HighSpeed::Off must compress the jpg-named file");
    assert!(jpg.csize < jpg.size / 10);
}

// ------------------------------------------------------------------ 4. split volumes

#[test]
fn split_volumes_reopen_via_zip_and_z01() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("data.bin");
    write(&src, &noise(1 << 20, 99));
    let outdir = t.path().join("arc");
    let zip = outdir.join("data.zip");
    let opts = ZipOptions { split_size: Some(100 * 1024), ..Default::default() };
    let stats = pack(&[src.clone()], &zip, &opts);
    assert!(stats.volumes >= 10, "expected many volumes, got {}", stats.volumes);

    let mut names: Vec<String> = fs::read_dir(&outdir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    let mut want: Vec<String> = (1..stats.volumes).map(|i| format!("data.z{i:02}")).collect();
    want.push("data.zip".into());
    want.sort();
    assert_eq!(names, want, "volume file names (no .part may remain)");
    for i in 1..stats.volumes {
        let len = fs::metadata(outdir.join(format!("data.z{i:02}"))).unwrap().len();
        assert!(len <= 100 * 1024, "volume {i} is {len} bytes");
    }

    let original = fs::read(&src).unwrap();
    for (n, reopen) in [zip.clone(), outdir.join("data.z01")].iter().enumerate() {
        let arc = open(reopen);
        assert!(arc.spanned, "{reopen:?} should be opened as a spanned archive");
        assert!(test_archive(&arc, None, &cancel(), &progress()).unwrap().iter().all(|(_, r)| r.is_ok()));
        let dest = t.path().join(format!("dest{n}"));
        extract(&arc, &ExtractOptions::new(&dest), &cancel(), &progress()).unwrap();
        assert_eq!(fs::read(dest.join("data.bin")).unwrap(), original, "reopen via {reopen:?}");
    }
}

#[test]
fn split_size_below_minimum_is_rejected() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("f.txt");
    write(&src, b"x");
    let entries = enumerate_inputs(&[src]);
    let opts = ZipOptions { split_size: Some(50 * 1024), ..Default::default() };
    let r = create_zip(&t.path().join("s.zip"), &entries, &opts, &cancel(), &progress());
    assert_eq!(kind_of(r), ErrorKind::InvalidSplitSize);
}

// ------------------------------------------------------------------ 5. large streaming

fn pattern(n: usize) -> Vec<u8> {
    let block: Vec<u8> = (0..4096u32).map(|i| b'a' + (i % 26) as u8).collect();
    block.iter().cycle().take(n).copied().collect()
}

fn large_roundtrip(method: Method, encryption: Encryption) {
    let is_aes = matches!(encryption, Encryption::Aes { .. });
    let label = format!("{method:?} aes={is_aes}");
    let t = TempDir::new().unwrap();
    // Not ".bin": that extension is on the default skip list and would be stored under HighSpeed::On.
    let src = t.path().join("big.txt");
    let data = pattern(20 << 20);
    write(&src, &data);
    let zip = t.path().join("big.zip");
    let opts = ZipOptions { method, level: Level::Normal, encryption: encryption.clone(), ..Default::default() };
    pack(&[src.clone()], &zip, &opts);

    let arc = open(&zip);
    let e = arc.entries.iter().find(|e| e.name == "big.txt").unwrap();
    assert_eq!(e.size, data.len() as u64, "{label}");
    if !is_aes {
        assert_eq!(e.crc, crc32fast::hash(&data), "{label}: stored CRC");
        if method != Method::Store {
            assert!(e.csize < e.size / 10, "{label}: not compressed ({} -> {})", e.size, e.csize);
        }
    }
    let r = test_archive(&arc, if is_aes { Some(PW) } else { None }, &cancel(), &progress()).unwrap();
    assert!(r.iter().all(|(_, r)| r.is_ok()), "{label}: {r:?}");

    let dest = t.path().join("dest");
    let o = ExtractOptions { password: is_aes.then(|| PW.to_string()), ..ExtractOptions::new(&dest) };
    let rep = extract(&arc, &o, &cancel(), &progress()).unwrap();
    assert!(rep.failed.is_empty(), "{label}: {:?}", rep.failed);
    assert!(fs::read(dest.join("big.txt")).unwrap() == data, "{label}: content differs");
}

#[test]
fn large_entry_deflate() {
    large_roundtrip(Method::Deflate, Encryption::None);
}

#[test]
fn large_entry_store() {
    large_roundtrip(Method::Store, Encryption::None);
}

#[test]
fn large_entry_zstd() {
    large_roundtrip(Method::Zstd, Encryption::None);
}

#[test]
fn large_entry_bzip2() {
    large_roundtrip(Method::Bzip2, Encryption::None);
}

#[test]
fn large_entry_aes() {
    large_roundtrip(Method::Deflate, Encryption::Aes { strength: 3, password: PW.into() });
}

// ------------------------------------------------------------------ 6. Windows rules

#[test]
fn windows_rules_on_the_wire() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("proj");
    write(&src.join("ä中文.txt"), b"unicode name");
    write(&src.join("plain.txt"), b"ascii");
    write(&src.join("sub/inner.txt"), b"inner");
    fs::create_dir_all(src.join("empty")).unwrap();
    write(&src.join(".DS_Store"), b"x");
    write(&src.join("__MACOSX/._a.txt"), b"x");
    write(&src.join("._x"), b"x");
    let zip = t.path().join("win.zip");
    pack(&[src.clone()], &zip, &ZipOptions::default());

    let bytes = fs::read(&zip).unwrap();
    assert!(!contains_sig(&bytes, 0x0606_4b50), "ZIP64 end record present in a small archive");
    let cd = parse_cd(&bytes);
    let names: Vec<String> = cd.iter().map(|r| String::from_utf8(r.name.clone()).unwrap()).collect();

    assert!(names.iter().all(|n| !n.contains('\\')), "backslash separator: {names:?}");
    assert!(
        !names.iter().any(|n| n.contains("__MACOSX") || n.contains("._") || n.contains(".DS_Store")),
        "macOS junk in archive: {names:?}"
    );
    for want in ["proj/", "proj/plain.txt", "proj/sub/", "proj/sub/inner.txt", "proj/empty/", "proj/ä中文.txt"] {
        assert!(names.iter().any(|n| n == want), "{want} missing from {names:?}");
    }

    let uni = cd.iter().find(|r| r.name == "proj/ä中文.txt".as_bytes()).unwrap();
    assert!(uni.flags & 0x800 != 0, "EFS bit not set for non-ASCII name");
    let up = find_extra(&uni.extra, 0x7075).expect("0x7075 Unicode Path extra present");
    assert_eq!(up[0], 1, "0x7075 version");
    assert_eq!(&up[5..], "proj/ä中文.txt".as_bytes(), "embedded UTF-8 name");

    // Note: the writer sets EFS on every name in UTF-8 mode (valid per spec), so ASCII names carry it too.
    // Source files get whatever the umask gives them; the archive must carry that same mode.
    let file_perm = fs::metadata(src.join("plain.txt")).unwrap().permissions().mode() & 0o777;
    for r in &cd {
        let is_dir = r.name.ends_with(b"/");
        let mode = r.ext_attr >> 16;
        assert_eq!(r.ver_made >> 8, 3, "made by unix");
        if is_dir {
            assert!(r.ext_attr & 0x10 != 0, "DOS dir bit missing on {:?}", String::from_utf8_lossy(&r.name));
            assert_eq!(mode & 0o170000, 0o040000, "dir mode");
        } else {
            assert_eq!(r.ext_attr & 0x10, 0, "dir bit on file");
            assert_eq!(mode & 0o170000, 0o100000, "file mode");
            assert_eq!(mode & 0o777, file_perm, "file perms");
        }
    }
}

#[test]
fn legacy_gbk_mode_has_no_efs_and_decodes_back() {
    let t = TempDir::new().unwrap();
    let name = "中文文件名称测试.txt";
    let src = t.path().join(name);
    write(&src, b"gbk body");
    let zip = t.path().join("legacy.zip");
    let opts = ZipOptions { legacy_codepage: Some("gbk".into()), ..Default::default() };
    pack(&[src], &zip, &opts);

    let bytes = fs::read(&zip).unwrap();
    let cd = parse_cd(&bytes);
    assert_eq!(cd.len(), 1);
    assert_eq!(cd[0].flags & 0x800, 0, "legacy mode must not set EFS");
    assert!(find_extra(&cd[0].extra, 0x7075).is_none(), "legacy mode must not write 0x7075");
    assert_ne!(cd[0].name, name.as_bytes(), "name should be GBK bytes, not UTF-8");

    let arc = open(&zip);
    assert_eq!(arc.entries[0].name, name, "auto-detect should decode the GBK name");
    let dest = t.path().join("out");
    unpack(&zip, &dest);
    assert_eq!(fs::read(dest.join(name)).unwrap(), b"gbk body");
}

#[test]
fn unrepresentable_name_in_legacy_codepage_is_an_error() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("emoji 😀.txt");
    write(&src, b"x");
    let entries = enumerate_inputs(std::slice::from_ref(&src));
    let zip = t.path().join("bad.zip");
    let opts = ZipOptions { legacy_codepage: Some("gbk".into()), ..Default::default() };
    let r = create_zip(&zip, &entries, &opts, &cancel(), &progress());
    assert_eq!(kind_of(r), ErrorKind::InvalidInput);
}

#[test]
fn failed_create_leaves_no_output_archive() {
    // A failing entry (unrepresentable name) must not leave a finished-looking archive behind.
    let t = TempDir::new().unwrap();
    let src = t.path().join("emoji 😀.txt");
    write(&src, b"x");
    let entries = enumerate_inputs(std::slice::from_ref(&src));
    let zip = t.path().join("bad.zip");
    let opts = ZipOptions { legacy_codepage: Some("gbk".into()), ..Default::default() };
    let _ = create_zip(&zip, &entries, &opts, &cancel(), &progress());
    assert!(!zip.exists(), "failed create left {zip:?} behind");
    assert!(snapshot(t.path()).keys().all(|k| !k.ends_with(".part")));
}

// ------------------------------------------------------------------ 7. hostile names

#[test]
fn traversal_names_are_refused_and_never_escape() {
    for smart in [false, true] {
        let t = TempDir::new().unwrap();
        let arc_dir = t.path().join("arc");
        let out = t.path().join("out");
        let zip = arc_dir.join("evil.zip");
        write(
            &zip,
            &raw_zip(&[
                ("../evil.txt", b"E1"),
                ("a/../../evil.txt", b"E2"),
                ("C:\\evil.txt", b"E3"),
                ("ok\\..\\..\\x", b"E4"),
                ("ok_normal.txt", b"NORMAL"),
            ]),
        );
        let arc = open(&zip);
        let o = ExtractOptions { smart, ..ExtractOptions::new(&out) };
        let rep = extract(&arc, &o, &cancel(), &progress()).unwrap();

        // Nothing escaped the sandbox: the only top-level dirs are the two we made.
        let mut top: Vec<String> = fs::read_dir(t.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        top.sort();
        assert_eq!(top, vec!["arc", "out"], "smart={smart}");
        for leaked in ["evil.txt", "x"] {
            assert!(!t.path().join(leaked).exists(), "smart={smart}: {leaked} escaped");
        }
        assert!(!arc_dir.join("evil.txt").exists());
        assert!(!arc_dir.join("x").exists());
        let escaped: Vec<_> = snapshot(t.path()).into_iter().filter(|(k, _)| !k.starts_with("out") && !k.starts_with("arc")).collect();
        assert!(escaped.is_empty(), "smart={smart}: stray files {escaped:?}");

        for bad in ["../evil.txt", "a/../../evil.txt", "C:/evil.txt", "ok/../../x"] {
            assert!(rep.unsafe_refused.iter().any(|r| r == bad), "smart={smart}: {bad} not reported in {:?}", rep.unsafe_refused);
        }
        let normal = if smart { find_file(&out, "ok_normal.txt") } else { out.join("ok_normal.txt") };
        assert_eq!(fs::read(&normal).unwrap(), b"NORMAL", "smart={smart}: normal entry not extracted");
    }
}

fn find_file(root: &Path, name: &str) -> PathBuf {
    snapshot(root)
        .into_iter()
        .find(|(k, _)| k.ends_with(name))
        .map(|(k, _)| root.join(k))
        .unwrap_or_else(|| panic!("{name} not found under {root:?}"))
}

#[test]
fn absolute_entry_name_is_refused() {
    let t = TempDir::new().unwrap();
    let zip = t.path().join("abs.zip");
    write(&zip, &raw_zip(&[("/abs.txt", b"E")]));
    let out = t.path().join("out");
    let o = ExtractOptions { smart: false, ..ExtractOptions::new(&out) };
    let rep = extract(&open(&zip), &o, &cancel(), &progress()).unwrap();
    assert!(rep.unsafe_refused.iter().any(|r| r.contains("abs.txt")), "not refused: {:?}", rep.unsafe_refused);
}

#[test]
fn backslash_names_become_forward_slash_paths() {
    let t = TempDir::new().unwrap();
    let zip = t.path().join("bs.zip");
    write(&zip, &raw_zip(&[("dir\\file.txt", b"hi"), ("dir\\sub\\deep.txt", b"deep")]));
    let arc = open(&zip);
    let names: Vec<&str> = arc.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["dir/file.txt", "dir/sub/deep.txt"]);
    let out = t.path().join("out");
    let o = ExtractOptions { smart: false, ..ExtractOptions::new(&out) };
    extract(&arc, &o, &cancel(), &progress()).unwrap();
    assert_eq!(fs::read(out.join("dir/file.txt")).unwrap(), b"hi");
    assert_eq!(fs::read(out.join("dir/sub/deep.txt")).unwrap(), b"deep");
}

// ------------------------------------------------------------------ 8. smart extract shapes

#[test]
fn smart_extract_single_root_folder_is_not_wrapped() {
    let t = TempDir::new().unwrap();
    let proj = t.path().join("proj");
    write(&proj.join("a.txt"), b"A");
    write(&proj.join("sub/b.txt"), b"B");
    let zip = t.path().join("single.zip");
    pack(&[proj], &zip, &ZipOptions::default());
    let out = t.path().join("out");
    unpack(&zip, &out);
    assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"A");
    assert_eq!(fs::read(out.join("sub/b.txt")).unwrap(), b"B");
    assert!(!out.join("proj").exists(), "root folder should be stripped");
}

#[test]
fn smart_extract_multi_root_is_wrapped_in_archive_stem() {
    let t = TempDir::new().unwrap();
    write(&t.path().join("x.txt"), b"X");
    write(&t.path().join("d/y.txt"), b"Y");
    let zip = t.path().join("bundle.zip");
    pack(&[t.path().join("x.txt"), t.path().join("d")], &zip, &ZipOptions::default());
    let out = t.path().join("out");
    unpack(&zip, &out);
    assert_eq!(fs::read(out.join("bundle/x.txt")).unwrap(), b"X");
    assert_eq!(fs::read(out.join("bundle/d/y.txt")).unwrap(), b"Y");
}

#[test]
fn smart_extract_single_file_wrap_option() {
    let t = TempDir::new().unwrap();
    let f = t.path().join("note.txt");
    write(&f, b"N");
    let zip = t.path().join("single.zip");
    pack(&[f], &zip, &ZipOptions::default());

    let out = t.path().join("plain");
    unpack(&zip, &out);
    assert_eq!(fs::read(out.join("note.txt")).unwrap(), b"N");

    let out2 = t.path().join("wrapped");
    let o = ExtractOptions { wrap_single_files: true, ..ExtractOptions::new(&out2) };
    extract(&open(&zip), &o, &cancel(), &progress()).unwrap();
    assert_eq!(fs::read(out2.join("single/note.txt")).unwrap(), b"N");
}

#[test]
fn smart_extract_root_plus_stray_file_is_wrapped() {
    let t = TempDir::new().unwrap();
    let proj = t.path().join("proj");
    write(&proj.join("a.txt"), b"A");
    write(&t.path().join("stray.txt"), b"S");
    let zip = t.path().join("mixed.zip");
    pack(&[proj, t.path().join("stray.txt")], &zip, &ZipOptions::default());
    let out = t.path().join("out");
    unpack(&zip, &out);
    assert_eq!(fs::read(out.join("mixed/proj/a.txt")).unwrap(), b"A");
    assert_eq!(fs::read(out.join("mixed/stray.txt")).unwrap(), b"S");
}

#[test]
fn smart_extract_dot_slash_prefixed_names() {
    let t = TempDir::new().unwrap();
    let zip = t.path().join("dot.zip");
    write(&zip, &raw_zip(&[("./d/", b""), ("./d/b.txt", b"B")]));
    let out = t.path().join("out");
    unpack(&zip, &out);
    assert_eq!(fs::read(out.join("b.txt")).unwrap(), b"B", "single './d' root should be stripped");
    assert!(!out.join("d").exists());
}

// ------------------------------------------------------------------ 9. cancellation

#[test]
fn cancel_before_create_leaves_no_files() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("proj");
    build_tree(&src);
    let entries = enumerate_inputs(&[src]);
    let outdir = t.path().join("out");
    fs::create_dir_all(&outdir).unwrap();
    let token = CancelToken::new();
    token.cancel();
    let r = create_zip(&outdir.join("c.zip"), &entries, &ZipOptions::default(), &token, &progress());
    assert_eq!(kind_of(r), ErrorKind::Cancelled);
    assert!(snapshot(&outdir).is_empty(), "files left behind: {:?}", snapshot(&outdir).keys().collect::<Vec<_>>());
}

#[test]
fn cancel_before_extract_leaves_no_files() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("proj");
    build_tree(&src);
    let zip = t.path().join("c.zip");
    pack(&[src], &zip, &ZipOptions::default());
    let dest = t.path().join("dest");
    let token = CancelToken::new();
    token.cancel();
    let o = ExtractOptions { delete_partial_on_cancel: true, ..ExtractOptions::new(&dest) };
    let r = extract(&open(&zip), &o, &token, &progress());
    assert_eq!(kind_of(r), ErrorKind::Cancelled);
    let files: Vec<String> = snapshot(&dest)
        .into_iter()
        .filter(|(_, n)| !matches!(n, Node::Dir))
        .map(|(k, _)| k)
        .collect();
    assert!(files.is_empty(), "files left behind: {files:?}");
}

// ------------------------------------------------------------------ 10. overwrite policies

fn overwrite_setup(t: &TempDir) -> (PathBuf, PathBuf) {
    let src = t.path().join("f.txt");
    write(&src, b"NEW CONTENT");
    let zip = t.path().join("f.zip");
    pack(&[src], &zip, &ZipOptions::default());
    let dest = t.path().join("dest");
    write(&dest.join("f.txt"), b"OLD CONTENT");
    (zip, dest)
}

#[test]
fn overwrite_skip() {
    let t = TempDir::new().unwrap();
    let (zip, dest) = overwrite_setup(&t);
    let o = ExtractOptions { overwrite: Overwrite::Skip, ..ExtractOptions::new(&dest) };
    let rep = extract(&open(&zip), &o, &cancel(), &progress()).unwrap();
    assert_eq!(rep.skipped, 1);
    assert_eq!(fs::read(dest.join("f.txt")).unwrap(), b"OLD CONTENT");
}

#[test]
fn overwrite_overwrite() {
    let t = TempDir::new().unwrap();
    let (zip, dest) = overwrite_setup(&t);
    let o = ExtractOptions { overwrite: Overwrite::Overwrite, ..ExtractOptions::new(&dest) };
    extract(&open(&zip), &o, &cancel(), &progress()).unwrap();
    assert_eq!(fs::read(dest.join("f.txt")).unwrap(), b"NEW CONTENT");
    assert!(!dest.join("f (1).txt").exists());
}

#[test]
fn overwrite_rename() {
    let t = TempDir::new().unwrap();
    let (zip, dest) = overwrite_setup(&t);
    let o = ExtractOptions { overwrite: Overwrite::Rename, ..ExtractOptions::new(&dest) };
    extract(&open(&zip), &o, &cancel(), &progress()).unwrap();
    assert_eq!(fs::read(dest.join("f.txt")).unwrap(), b"OLD CONTENT");
    assert_eq!(fs::read(dest.join("f (1).txt")).unwrap(), b"NEW CONTENT");
}

#[test]
fn overwrite_ask_uses_callback() {
    for choice in [ConflictChoice::Skip, ConflictChoice::Overwrite, ConflictChoice::Rename] {
        let t = TempDir::new().unwrap();
        let (zip, dest) = overwrite_setup(&t);
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        let cb: ConflictFn = Arc::new(move |_p: &Path, _e: Option<&ZipEntry>| {
            c2.fetch_add(1, Ordering::SeqCst);
            choice
        });
        let o = ExtractOptions { overwrite: Overwrite::Ask, on_conflict: Some(cb), ..ExtractOptions::new(&dest) };
        extract(&open(&zip), &o, &cancel(), &progress()).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{choice:?}: callback calls");
        match choice {
            ConflictChoice::Skip => assert_eq!(fs::read(dest.join("f.txt")).unwrap(), b"OLD CONTENT"),
            ConflictChoice::Overwrite => assert_eq!(fs::read(dest.join("f.txt")).unwrap(), b"NEW CONTENT"),
            ConflictChoice::Rename => {
                assert_eq!(fs::read(dest.join("f.txt")).unwrap(), b"OLD CONTENT");
                assert_eq!(fs::read(dest.join("f (1).txt")).unwrap(), b"NEW CONTENT");
            }
        }
    }
}

// ------------------------------------------------------------------ 11. corruption

#[test]
fn flipped_stored_byte_is_a_crc_mismatch() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("payload.txt");
    write(&src, b"START-UNIQUE-PAYLOAD-0123456789-END");
    let zip = t.path().join("good.zip");
    let opts = ZipOptions { method: Method::Store, level: Level::Normal, ..Default::default() };
    pack(&[src], &zip, &opts);

    let mut bytes = fs::read(&zip).unwrap();
    let pos = bytes.windows(6).position(|w| w == b"UNIQUE").expect("payload is stored verbatim");
    bytes[pos] ^= 0x01;
    let bad = t.path().join("bad.zip");
    fs::write(&bad, &bytes).unwrap();

    let arc = open(&bad);
    let results = test_archive(&arc, None, &cancel(), &progress()).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(kind_of(results.into_iter().next().unwrap().1), ErrorKind::CrcMismatch);
}

#[test]
fn truncated_archive_fails_to_open_without_panic() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("f.txt");
    write(&src, &text(10_000));
    let zip = t.path().join("ok.zip");
    pack(&[src], &zip, &ZipOptions::default());
    let full = fs::read(&zip).unwrap();
    for cut in [0usize, 10, 30, full.len() / 2, full.len() - 1] {
        let p = t.path().join(format!("cut{cut}.zip"));
        fs::write(&p, &full[..cut]).unwrap();
        assert!(ZipArchive::open(&p, ReadOptions::default()).is_err(), "truncated to {cut} bytes opened");
    }
}

// ------------------------------------------------------------------ 12. SFX prefix

#[test]
fn sfx_prefixed_archive_opens_and_extracts() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("hello.txt");
    write(&src, b"hello from sfx");
    let zip = t.path().join("plain.zip");
    pack(&[src], &zip, &ZipOptions::default());
    let mut bytes = vec![b'A'; 5000]; // junk stub without ZIP signatures
    bytes.extend(fs::read(&zip).unwrap());
    let sfx = t.path().join("app.exe");
    fs::write(&sfx, &bytes).unwrap();

    let arc = open(&sfx);
    assert_eq!(arc.entries.len(), 1);
    let dest = t.path().join("out");
    extract(&arc, &ExtractOptions::new(&dest), &cancel(), &progress()).unwrap();
    assert_eq!(fs::read(dest.join("hello.txt")).unwrap(), b"hello from sfx");
}

// ------------------------------------------------------------------ 13. comment

#[test]
fn zip_comment_roundtrips() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("f.txt");
    write(&src, b"c");
    let zip = t.path().join("c.zip");
    let comment = "fastzip round-trip comment";
    let opts = ZipOptions { comment: comment.into(), ..Default::default() };
    pack(&[src], &zip, &opts);
    let arc = open(&zip);
    assert_eq!(arc.comment, comment.as_bytes());
    unpack(&zip, &t.path().join("out"));
}

// ------------------------------------------------------------------ 14. reproducibility

#[test]
fn identical_inputs_produce_identical_bytes() {
    let t = TempDir::new().unwrap();
    let src = t.path().join("proj");
    build_tree(&src);
    write(&src.join("more/big.bin"), &noise(2 << 20, 5));
    let entries = enumerate_inputs(&[src]);
    let opts = ZipOptions { encryption: Encryption::None, ..Default::default() };
    let a = t.path().join("a.zip");
    let b = t.path().join("b.zip");
    create_zip(&a, &entries, &opts, &cancel(), &progress()).unwrap();
    create_zip(&b, &entries, &opts, &cancel(), &progress()).unwrap();
    assert!(fs::read(&a).unwrap() == fs::read(&b).unwrap(), "archives differ between runs");
}
