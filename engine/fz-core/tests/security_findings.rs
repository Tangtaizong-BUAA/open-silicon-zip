//! Executable attacks for the medium-severity 2026-10 security findings.
//! Inputs are synthetic and all outputs stay in private temporary directories.
use fz_core::{CancelToken, Progress};
use fz_core::zip::{self, ReadOptions, ZipArchive, ExtractOptions, Overwrite, ConflictChoice, ZipOptions};
use fz_core::formats::{sevenz, tar::list_tar};
use std::{fs, io::{Cursor, Read, Write}, path::Path, sync::Arc};

fn u16le(v: &mut Vec<u8>, n: u16) { v.extend(n.to_le_bytes()); }
fn u32le(v: &mut Vec<u8>, n: u32) { v.extend(n.to_le_bytes()); }

fn zip_bytes(name: &str, data: &[u8], mode: u32, copies: usize, declared: u16) -> Vec<u8> {
    let crc = crc32fast::hash(data);
    let mut out = Vec::new();
    u32le(&mut out, 0x04034b50);
    for n in [20, 0x800, 0, 0, 0] { u16le(&mut out, n); }
    for n in [crc, data.len() as u32, data.len() as u32] { u32le(&mut out, n); }
    u16le(&mut out, name.len() as u16); u16le(&mut out, 0);
    out.extend(name.as_bytes()); out.extend(data);
    let cd_start = out.len();
    for _ in 0..copies {
        u32le(&mut out, 0x02014b50);
        for n in [0x0314, 20, 0x800, 0, 0, 0] { u16le(&mut out, n); }
        for n in [crc, data.len() as u32, data.len() as u32] { u32le(&mut out, n); }
        for n in [name.len() as u16, 0, 0, 0, 0] { u16le(&mut out, n); }
        u32le(&mut out, mode << 16); u32le(&mut out, 0); out.extend(name.as_bytes());
    }
    let cd_size = out.len() - cd_start;
    u32le(&mut out, 0x06054b50);
    for n in [0, 0, declared, declared] { u16le(&mut out, n); }
    u32le(&mut out, cd_size as u32); u32le(&mut out, cd_start as u32); u16le(&mut out, 0);
    out
}

#[test]
fn zip_cardinality_is_checked_even_when_eocd_underreports() {
    let d = tempfile::tempdir().unwrap(); let path = d.path().join("dense.zip");
    fs::write(&path, zip_bytes("f", b"", 0o100644, 100_000, 0)).unwrap();
    assert_eq!(ZipArchive::open(&path, ReadOptions::default()).unwrap().entries.len(), 100_000);
    fs::write(&path, zip_bytes("f", b"", 0o100644, 100_001, 0)).unwrap();
    let e = ZipArchive::open(&path, ReadOptions::default()).err().expect("dense metadata must be bounded");
    assert!(e.to_string().contains("budget"), "{e}");
}

#[test]
fn zip_paths_and_parsed_metadata_are_bounded() {
    let d = tempfile::tempdir().unwrap(); let path = d.path().join("names.zip");
    for (name, copies) in [("a/".repeat(64) + "x", 1), ("a".repeat(4097), 1), ("a".repeat(4096), 2050)] {
        fs::write(&path, zip_bytes(&name, b"", 0o100644, copies, 0)).unwrap();
        let e = ZipArchive::open(&path, ReadOptions::default()).err().expect("name/metadata limit");
        assert!(e.to_string().contains("budget"), "{e}");
    }
}

#[cfg(unix)]
#[test]
fn zip_symlink_obeys_every_conflict_policy() {
    let d = tempfile::tempdir().unwrap(); let archive = d.path().join("link.zip");
    fs::write(&archive, zip_bytes("victim", b"safe", 0o120777, 1, 1)).unwrap();
    let a = ZipArchive::open(&archive, ReadOptions::default()).unwrap();
    for (i, policy, answer) in [
        (0, Overwrite::Skip, ConflictChoice::Skip), (1, Overwrite::Rename, ConflictChoice::Rename),
        (2, Overwrite::Ask, ConflictChoice::Skip), (3, Overwrite::Ask, ConflictChoice::Rename),
        (4, Overwrite::Ask, ConflictChoice::Overwrite), (5, Overwrite::Overwrite, ConflictChoice::Overwrite),
    ] {
        let dest = d.path().join(format!("out-{i}")); fs::create_dir(&dest).unwrap();
        let victim = dest.join("victim"); fs::write(&victim, b"ORIGINAL").unwrap();
        let mut o = ExtractOptions::new(&dest); o.smart = false; o.overwrite = policy;
        o.on_conflict = Some(Arc::new(move |_, e| { assert!(e.unwrap().is_symlink); answer }));
        let report = zip::extract(&a, &o, &CancelToken::new(), &Progress::new()).unwrap();
        if matches!(answer, ConflictChoice::Overwrite) {
            assert_eq!(fs::read_link(&victim).unwrap(), Path::new("safe"));
        } else {
            assert_eq!(fs::read(&victim).unwrap(), b"ORIGINAL");
            if matches!(answer, ConflictChoice::Rename) {
                assert_eq!(fs::read_link(dest.join("victim (1)")).unwrap(), Path::new("safe"));
            } else { assert_eq!(report.skipped, 1); }
        }
    }
}

#[cfg(unix)]
#[test]
fn invalid_symlink_never_deletes_existing_leaf() {
    let d = tempfile::tempdir().unwrap(); let archive = d.path().join("invalid.zip");
    fs::write(&archive, zip_bytes("victim", b"bad\0target", 0o120777, 1, 1)).unwrap();
    let a = ZipArchive::open(&archive, ReadOptions::default()).unwrap();
    let dest = d.path().join("out"); fs::create_dir(&dest).unwrap(); fs::write(dest.join("victim"), b"ORIGINAL").unwrap();
    let mut o = ExtractOptions::new(&dest); o.smart = false; o.overwrite = Overwrite::Overwrite;
    o.allow_external_symlinks = true; // reach the failing OS link creation
    let report = zip::extract(&a, &o, &CancelToken::new(), &Progress::new()).unwrap();
    assert!(!report.failed.is_empty());
    assert_eq!(fs::read(dest.join("victim")).unwrap(), b"ORIGINAL");
}

#[cfg(unix)]
#[test]
fn output_symlinks_cannot_redirect_edit_repair_join_or_split() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap(); let original = d.path().join("original");
    fs::write(&original, b"ORIGINAL").unwrap();
    let archive = d.path().join("input.zip"); fs::write(&archive, zip_bytes("f", b"payload", 0o100644, 1, 1)).unwrap();
    symlink(&original, d.path().join("input.zip.edit.part")).unwrap();
    symlink(&original, d.path().join("input.zip.add.part")).unwrap();
    zip::edit::edit_zip(&archive, &zip::edit::EditOp::default(), &ZipOptions::default(), &CancelToken::new(), &Progress::new()).unwrap();
    assert_eq!(fs::read(&original).unwrap(), b"ORIGINAL");
    let repair = d.path().join("repaired.zip"); symlink(&original, &repair).unwrap();
    assert!(zip::repair::repair_zip(&archive, &repair, &CancelToken::new()).is_err());
    let out = d.path().join("split"); symlink(&original, d.path().join("split.001")).unwrap();
    assert!(sevenz::split_file(&archive, &out, 16).is_err());
    let first = d.path().join("joined.001"); fs::write(&first, b"bytes").unwrap();
    symlink(&original, d.path().join("joined")).unwrap();
    assert!(sevenz::join_volumes(&first, d.path()).is_err());
    assert_eq!(fs::read(&original).unwrap(), b"ORIGINAL");
    assert!(fs::symlink_metadata(&repair).unwrap().file_type().is_symlink());
}

#[test]
fn new_output_operations_refuse_existing_regular_files() {
    let d = tempfile::tempdir().unwrap(); let archive = d.path().join("in.zip");
    fs::write(&archive, zip_bytes("f", b"payload", 0o100644, 1, 1)).unwrap();
    let repair = d.path().join("out.zip"); fs::write(&repair, b"ORIGINAL").unwrap();
    assert!(zip::repair::repair_zip(&archive, &repair, &CancelToken::new()).is_err());
    assert_eq!(fs::read(&repair).unwrap(), b"ORIGINAL");
    let out = d.path().join("split"); fs::write(d.path().join("split.001"), b"ORIGINAL").unwrap();
    assert!(sevenz::split_file(&archive, &out, 20).is_err());
    assert_eq!(fs::read(d.path().join("split.001")).unwrap(), b"ORIGINAL");
    let first = d.path().join("joined.001"); fs::write(&first, b"new").unwrap();
    fs::write(d.path().join("joined"), b"ORIGINAL").unwrap();
    assert!(sevenz::join_volumes(&first, d.path()).is_err());
    assert_eq!(fs::read(d.path().join("joined")).unwrap(), b"ORIGINAL");
}

fn tar_bytes(name: &str, size: usize, count: usize) -> Vec<u8> {
    let mut data = Vec::new();
    { let mut builder = tar::Builder::new(&mut data);
      for _ in 0..count { let mut h = tar::Header::new_gnu(); h.set_size(size as u64); h.set_mode(0o644); h.set_cksum();
        builder.append_data(&mut h, name, std::io::repeat(0).take(size as u64)).unwrap(); }
      builder.finish().unwrap(); }
    data
}

#[test]
fn compressed_tar_listing_has_decoded_byte_and_metadata_limits() {
    let d = tempfile::tempdir().unwrap(); let path = d.path().join("bomb.tgz");
    let data = tar_bytes("f", 128 << 10, 1);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(&data).unwrap(); fs::write(&path, gz.finish().unwrap()).unwrap();
    let e = list_tar(&path, 4096, &CancelToken::new()).err().expect("malicious input must be rejected");
    assert!(e.to_string().contains("budget"), "{e}");
    // GNU extension gets buffered inside tar::Archive, before our entry loop.
    let long = tar_bytes(&"x".repeat(100_000), 0, 1);
    fs::write(&path, long).unwrap();
    let e = list_tar(&path, 1 << 30, &CancelToken::new()).err().expect("malicious input must be rejected");
    assert!(e.to_string().contains("budget"), "{e}");
}

#[test]
fn tar_listing_rejects_header_flood_and_preserves_long_name_support() {
    let d = tempfile::tempdir().unwrap(); let path = d.path().join("flood.tar");
    fs::write(&path, tar_bytes("f", 0, 100_001)).unwrap();
    let e = list_tar(&path, 1 << 30, &CancelToken::new()).err().expect("malicious input must be rejected");
    assert!(e.to_string().contains("budget"), "{e}");
    let name = "sub/".to_string() + &"a".repeat(200);
    fs::write(&path, tar_bytes(&name, 128 << 10, 1)).unwrap();
    let entries = list_tar(&path, 1 << 30, &CancelToken::new()).unwrap();
    assert_eq!(entries[0].name, name); assert_eq!(entries[0].size, 128 << 10);
}

fn number(out: &mut Vec<u8>, value: u64) {
    let mut first = 0u8; let mut mask = 0x80;
    let mut low = Vec::new();
    for i in 0..8 { if value < (1u64 << (7 * (i + 1))) { first |= (value >> (8 * i)) as u8; break; }
        first |= mask; mask >>= 1; low.push((value >> (8 * i)) as u8); }
    out.push(first); out.extend(low);
}

fn raw_7z(packed: &[u8], next_header: &[u8]) -> Vec<u8> {
    let mut start = Vec::new(); start.extend((packed.len() as u64).to_le_bytes());
    start.extend((next_header.len() as u64).to_le_bytes()); start.extend(crc32fast::hash(next_header).to_le_bytes());
    let mut out = vec![0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c, 0, 4];
    out.extend(crc32fast::hash(&start).to_le_bytes()); out.extend(start); out.extend(packed); out.extend(next_header); out
}

fn streams(out: &mut Vec<u8>, packed_size: u64, size: u64, method: &[u8], crc: Option<u32>) {
    out.extend([6, 0, 1, 9]); number(out, packed_size); out.extend([0, 7, 11, 1, 0, 1, method.len() as u8]);
    out.extend(method); out.push(12); number(out, size);
    if let Some(crc) = crc { out.extend([10, 1]); out.extend(crc.to_le_bytes()); }
    out.extend([0, 8, 0, 0]);
}

fn huge_7z(size: u64) -> Vec<u8> {
    let mut h = vec![1, 4]; streams(&mut h, 1, size, &[0], Some(crc32fast::hash(b"x")));
    h.extend([5, 1, 17]); number(&mut h, 15); h.push(0);
    for c in "victim\0".encode_utf16() { h.extend(c.to_le_bytes()); } h.extend([0, 0]); raw_7z(b"x", &h)
}

#[test]
fn sevenz_huge_lengths_cannot_succeed_or_replace_original() {
    for size in [i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX] {
        let d = tempfile::tempdir().unwrap(); let path = d.path().join("huge.7z"); fs::write(&path, huge_7z(size)).unwrap();
        let mut reader = sevenz_rust2::ArchiveReader::open(&path, sevenz_rust2::Password::empty()).unwrap();
        assert!(reader.for_each_entries(|_, r| { std::io::copy(r, &mut std::io::sink())?; Ok(true) }).is_err(), "size {size}");
        let dest = d.path().join("out"); fs::create_dir(&dest).unwrap(); fs::write(dest.join("victim"), b"ORIGINAL").unwrap();
        let mut o = ExtractOptions::new(&dest); o.smart = false; o.overwrite = Overwrite::Overwrite;
        assert!(sevenz::extract_7z(&path, &o, &CancelToken::new(), &Progress::new()).is_err());
        assert_eq!(fs::read(dest.join("victim")).unwrap(), b"ORIGINAL");
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 1, "temporary output leaked");
    }
}

#[test]
fn sevenz_encoded_header_and_read_file_enforce_operational_caps() {
    let mut deflate = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    deflate.write_all(&[1, 0]).unwrap();
    std::io::copy(&mut std::io::repeat(0).take((64 << 20) - 1), &mut deflate).unwrap();
    let packed = deflate.finish().unwrap();
    let mut h = vec![0x17]; streams(&mut h, packed.len() as u64, (64 << 20) + 1, &[4, 1, 8], None);
    let bytes = raw_7z(&packed, &h);
    let error = sevenz_rust2::ArchiveReader::new(Cursor::new(bytes), sevenz_rust2::Password::empty()).err().expect("decoded header cap");
    assert!(error.to_string().contains("resource budget"), "{error}");
    let bytes = huge_7z((256 << 20) + 1);
    let mut reader = sevenz_rust2::ArchiveReader::new(Cursor::new(bytes), sevenz_rust2::Password::empty()).unwrap();
    let error = reader.read_file("victim").err().expect("malicious input must be rejected");
    assert!(error.to_string().contains("resource budget"), "{error}");
}

#[test]
fn solid_read_has_cumulative_budget_but_streaming_accepts_large_files() {
    use sevenz_rust2::{ArchiveEntry, ArchiveReader, ArchiveWriter, EncoderConfiguration, EncoderMethod, Password, SourceReader};
    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(vec![EncoderConfiguration::new(EncoderMethod::DEFLATE)]);
    let sizes = [140u64 << 20, 140 << 20, 300 << 20];
    let entries = ["prefix", "target", "large"].iter().map(|n| ArchiveEntry::new_file(n)).collect();
    let sources = sizes.iter().map(|n| SourceReader::new(std::io::repeat(0).take(*n))).collect();
    writer.push_archive_entries(entries, sources).unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let mut reader = ArchiveReader::new(Cursor::new(&bytes), Password::empty()).unwrap();
    let error = reader.read_file("target").err().expect("malicious input must be rejected");
    assert!(error.to_string().contains("resource budget"), "{error}");
    let mut reader = ArchiveReader::new(Cursor::new(&bytes), Password::empty()).unwrap();
    let mut decoded = Vec::new();
    reader.for_each_entries(|entry, source| {
        let n = std::io::copy(source, &mut std::io::sink())?;
        assert_eq!(n, entry.size()); decoded.push(n); Ok(true)
    }).unwrap();
    assert_eq!(decoded, sizes);
}

#[cfg(unix)]
#[test]
fn public_sevenz_helper_rejects_existing_ancestor_and_preserves_corrupt_overwrite() {
    let d = tempfile::tempdir().unwrap(); let dest = d.path().join("out"); let outside = d.path().join("outside");
    fs::create_dir(&dest).unwrap(); fs::create_dir(&outside).unwrap(); fs::write(outside.join("victim"), b"ORIGINAL").unwrap();
    std::os::unix::fs::symlink(&outside, dest.join("link")).unwrap();
    let mut writer = sevenz_rust2::ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.push_archive_entry(sevenz_rust2::ArchiveEntry::new_file("link/victim"), Some(b"ATTACK".as_slice())).unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    assert!(sevenz_rust2::decompress(Cursor::new(bytes), &dest).is_err());
    assert_eq!(fs::read(outside.join("victim")).unwrap(), b"ORIGINAL");
    fs::write(dest.join("victim"), b"ORIGINAL").unwrap();
    assert!(sevenz_rust2::decompress(Cursor::new(huge_7z(2)), &dest).is_err());
    assert_eq!(fs::read(dest.join("victim")).unwrap(), b"ORIGINAL");
}

#[cfg(unix)]
#[test]
fn default_sevenz_helper_rechecks_ancestor_replaced_after_callback_dispatch() {
    let d = tempfile::tempdir().unwrap(); let dest = d.path().join("out"); let outside = d.path().join("outside");
    fs::create_dir_all(dest.join("sub")).unwrap(); fs::create_dir(&outside).unwrap();
    fs::write(outside.join("victim"), b"ORIGINAL").unwrap();
    let mut writer = sevenz_rust2::ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.push_archive_entry(sevenz_rust2::ArchiveEntry::new_file("sub/victim"), Some(b"ATTACK".as_slice())).unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let result = sevenz_rust2::decompress_with_extract_fn(Cursor::new(bytes), &dest, |entry, reader, path| {
        // A deterministic replacement after the outer lexical/path check.
        fs::rename(dest.join("sub"), dest.join("old-sub"))?;
        std::os::unix::fs::symlink(&outside, dest.join("sub"))?;
        sevenz_rust2::default_entry_extract_fn(entry, reader, path)
    });
    assert!(result.is_err());
    assert_eq!(fs::read(outside.join("victim")).unwrap(), b"ORIGINAL");
}
