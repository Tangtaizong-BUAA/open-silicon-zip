//! Cancellation latency and cleanup suite (06 §5.4).
//!
//! For each long-running operation the worker runs on its own thread; the main thread polls
//! `Progress::snapshot` every 1 ms and calls `CancelToken::cancel` once progress reaches
//! ~1%, ~50% and ~99% of the tree size. Asserts: the operation returns `Cancelled` (or `Ok`
//! if it had already finished) with latency < 500 ms; no output/`.part` files remain for
//! create ops; no files remain in the destination for extract ops; no new `fz-*` temp dirs.
//! Run with `--nocapture` to see the latency table.
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use fz_core::enumerate::{enumerate, EnumOptions};
use fz_core::formats::codecs::Codec;
use fz_core::formats::sevenz::{create_7z, extract_7z, SevenOptions, Solid, SzMethod};
use fz_core::formats::tar::{create_tar, extract_tar, TarOptions};
use fz_core::zip::{
    create_zip, extract, test_archive, Encryption, ExtractOptions, Method, ReadOptions, ZipArchive, ZipOptions,
};
use fz_core::{CancelToken, ErrorKind, Progress};
use tempfile::Builder;

const PW: &str = "correct horse battery";
const FILE_SZ: usize = 2 << 20; // 2 MiB per file, 32 files = 64 MiB
const NFILES: usize = 16;
const LATENCY_LIMIT: Duration = Duration::from_millis(500);
const TARGET: Duration = Duration::from_millis(100);

fn write(p: &Path, data: &[u8]) {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, data).unwrap();
}

fn text(n: usize) -> Vec<u8> {
    let line = b"The quick brown fox jumps over the lazy dog 0123456789\n";
    line.iter().cycle().take(n).copied().collect()
}

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

/// Builds `root/compressible/*.txt` (32 MiB) and `root/random/*.bin` (32 MiB). Returns total bytes.
fn build_tree(root: &Path) -> u64 {
    for i in 0..NFILES {
        write(&root.join(format!("compressible/doc-{i:02}.txt")), &text(FILE_SZ));
        write(&root.join(format!("random/blob-{i:02}.bin")), &noise(FILE_SZ, i as u64 + 1));
    }
    (2 * NFILES * FILE_SZ) as u64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    ZipDeflate,
    ZipAes,
    ZipExtract,
    ZipTest,
    TarGz,
    TarExtract,
    SzLzma2Solid,
    SzExtract,
}

impl Op {
    const ALL: [Op; 8] = [
        Op::ZipDeflate,
        Op::ZipAes,
        Op::ZipExtract,
        Op::ZipTest,
        Op::TarGz,
        Op::TarExtract,
        Op::SzLzma2Solid,
        Op::SzExtract,
    ];

    fn name(self) -> &'static str {
        match self {
            Op::ZipDeflate => "create_zip deflate",
            Op::ZipAes => "create_zip AES-256",
            Op::ZipExtract => "zip::extract",
            Op::ZipTest => "zip::test_archive",
            Op::TarGz => "create_tar gzip",
            Op::TarExtract => "extract_tar",
            Op::SzLzma2Solid => "create_7z LZMA2 solid",
            Op::SzExtract => "extract_7z",
        }
    }

    fn is_create(self) -> bool {
        matches!(self, Op::ZipDeflate | Op::ZipAes | Op::TarGz | Op::SzLzma2Solid)
    }

    fn is_extract(self) -> bool {
        matches!(self, Op::ZipExtract | Op::TarExtract | Op::SzExtract)
    }
}

struct Fx {
    src: PathBuf,
    zip_plain: PathBuf,
    tar_gz: PathBuf,
    sz: PathBuf,
}

/// Runs one operation. Create ops write into `dir/out.*`; extract ops write into `dir/dest`.
fn run(op: Op, fx: &Fx, dir: &Path, c: &CancelToken, p: &Progress) -> fz_core::Result<()> {
    let dest = dir.join("dest");
    match op {
        Op::ZipDeflate | Op::ZipAes => {
            let entries = enumerate(std::slice::from_ref(&fx.src), &EnumOptions::default(), &CancelToken::new())?;
            let mut o = ZipOptions { method: Method::Deflate, ..Default::default() };
            if op == Op::ZipAes {
                o.encryption = Encryption::Aes { strength: 3, password: PW.into() };
            }
            create_zip(&dir.join("out.zip"), &entries, &o, c, p).map(|_| ())
        }
        Op::ZipExtract => {
            let arc = ZipArchive::open(&fx.zip_plain, ReadOptions::default())?;
            extract(&arc, &ExtractOptions::new(dest.clone()), c, p).map(|_| ())
        }
        Op::ZipTest => {
            let arc = ZipArchive::open(&fx.zip_plain, ReadOptions::default())?;
            test_archive(&arc, None, c, p).map(|_| ())
        }
        Op::TarGz => {
            let entries = enumerate(std::slice::from_ref(&fx.src), &EnumOptions::default(), &CancelToken::new())?;
            let o = TarOptions { codec: Codec::Gzip, ..Default::default() };
            create_tar(&dir.join("out.tar.gz"), &entries, &o, c, p).map(|_| ())
        }
        Op::TarExtract => extract_tar(&fx.tar_gz, &ExtractOptions::new(dest.clone()), c, p).map(|_| ()),
        Op::SzLzma2Solid => {
            let entries = enumerate(std::slice::from_ref(&fx.src), &EnumOptions::default(), &CancelToken::new())?;
            let o = SevenOptions { method: SzMethod::Lzma2, solid: Solid::Single, ..Default::default() };
            create_7z(&dir.join("out.7z"), &entries, &o, c, p).map(|_| ())
        }
        Op::SzExtract => extract_7z(&fx.sz, &ExtractOptions::new(dest.clone()), c, p).map(|_| ()),
    }
}

/// All entries under `dir` (files and dirs), recursively.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        out.push(p.clone());
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            walk(&p, out);
        }
    }
}

fn leftover_files(dir: &Path) -> Vec<String> {
    let mut all = Vec::new();
    walk(dir, &mut all);
    all.into_iter()
        .filter(|p| !p.is_dir())
        .map(|p| p.strip_prefix(dir).unwrap_or(&p).display().to_string())
        .collect()
}

fn fz_dirs() -> HashSet<String> {
    let Ok(rd) = fs::read_dir(std::env::temp_dir()) else { return HashSet::new() };
    rd.flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("fz-"))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

struct Row {
    op: &'static str,
    pct: u32,
    latency: String,
    result: String,
    verdict: String,
}

#[test]
fn cancel_suite() {
    let base = Builder::new().prefix("cancel-suite-").tempdir().unwrap();
    let root = base.path().to_path_buf();

    // Fixture: 64 MiB tree plus archives produced uncancelled.
    let src = root.join("src");
    let total = build_tree(&src);
    let fixdir = root.join("fixtures");
    fs::create_dir_all(&fixdir).unwrap();
    let mut fx = Fx { src: src.clone(), zip_plain: PathBuf::new(), tar_gz: PathBuf::new(), sz: PathBuf::new() };
    let nc = CancelToken::new();
    let np = Progress::new();
    run(Op::ZipDeflate, &fx, &fixdir, &nc, &np).expect("fixture zip");
    fx.zip_plain = fixdir.join("plain.zip");
    fs::rename(fixdir.join("out.zip"), &fx.zip_plain).unwrap();
    run(Op::TarGz, &fx, &fixdir, &nc, &np).expect("fixture tar.gz");
    fx.tar_gz = fixdir.join("bundle.tar.gz");
    fs::rename(fixdir.join("out.tar.gz"), &fx.tar_gz).unwrap();
    run(Op::SzLzma2Solid, &fx, &fixdir, &nc, &np).expect("fixture 7z");
    fx.sz = fixdir.join("bundle.7z");
    fs::rename(fixdir.join("out.7z"), &fx.sz).unwrap();
    let fx = Arc::new(fx);

    let fz_before = fz_dirs();
    let mut rows: Vec<Row> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    let mut run_no = 0u32;

    for op in Op::ALL {
        for pct in [1u32, 50, 99] {
            run_no += 1;
            let dir = root.join(format!("run-{run_no:03}"));
            fs::create_dir_all(&dir).unwrap();
            let thr = total * pct as u64 / 100;
            eprintln!("[{:?}] start {} @{pct}%", Instant::now(), op.name());

            let c = Arc::new(CancelToken::new());
            let p = Arc::new(Progress::new());
            let (c2, p2, fx2, dir2) = (c.clone(), p.clone(), fx.clone(), dir.clone());
            let h = thread::spawn(move || {
                let r = run(op, &fx2, &dir2, &c2, &p2).map_err(|e| e.kind);
                (r, Instant::now())
            });

            let mut cancel_at: Option<Instant> = None;
            loop {
                if h.is_finished() {
                    break;
                }
                let s = p.snapshot();
                if s.bytes_read.max(s.bytes_written) >= thr {
                    cancel_at = Some(Instant::now());
                    c.cancel();
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
            let (res, done_at) = h.join().expect("worker panicked");
            let name = op.name();
            let label = format!("{name} @{pct}%");
            let (latency, verdict) = match (&res, cancel_at) {
                (Ok(()), None) => ("-".to_string(), "OK (finished before threshold)".to_string()),
                (Ok(()), Some(_)) => ("-".to_string(), "OK (finished before cancel took effect)".to_string()),
                (Err(k), None) => {
                    violations.push(format!("{label}: error {k:?} without cancel"));
                    ("-".into(), format!("VIOLATION: {k:?}"))
                }
                (Err(k), Some(t0)) => {
                    let lat = done_at.saturating_duration_since(t0);
                    let mut v = Vec::new();
                    if *k != ErrorKind::Cancelled {
                        v.push(format!("returned {k:?}, expected Cancelled"));
                    }
                    if lat >= LATENCY_LIMIT {
                        v.push(format!("latency {lat:?} >= 500ms"));
                    }
                    if op.is_create() {
                        let left = leftover_files(&dir);
                        if !left.is_empty() {
                            v.push(format!("leftover after cancel: {left:?}"));
                        }
                    } else if op.is_extract() {
                        let left = leftover_files(&dest_of(&dir));
                        if !left.is_empty() {
                            v.push(format!("files left in dest: {:?}", &left[..left.len().min(5)]));
                        }
                    }
                    let verdict = if v.is_empty() {
                        if lat > TARGET { "OK (over 100ms target, under 500ms)".into() } else { "OK".into() }
                    } else {
                        for x in &v {
                            violations.push(format!("{label}: {x}"));
                        }
                        format!("VIOLATION: {}", v.join("; "))
                    };
                    (format!("{:.1}", lat.as_secs_f64() * 1000.0), verdict)
                }
            };
            let result = match &res {
                Ok(()) => "Ok".to_string(),
                Err(k) => format!("{k:?}"),
            };
            rows.push(Row { op: name, pct, latency, result, verdict });
            let _ = fs::remove_dir_all(&dir);
        }
    }

    let new_fz: Vec<String> = fz_dirs().difference(&fz_before).cloned().collect();
    if !new_fz.is_empty() {
        violations.push(format!("leftover fz-* temp dirs: {new_fz:?}"));
    }

    eprintln!("\n{:<24} {:>5} {:>12}  {:<22} {}", "operation", "pct", "latency_ms", "result", "verdict");
    for r in &rows {
        eprintln!("{:<24} {:>4}% {:>12}  {:<22} {}", r.op, r.pct, r.latency, r.result, r.verdict);
    }
    eprintln!("total bytes: {total}; violations: {}", violations.len());

    assert!(violations.is_empty(), "cancellation violations:\n{}", violations.join("\n"));
}

/// Destination directory used by extract ops in a run directory.
fn dest_of(dir: &Path) -> PathBuf {
    dir.join("dest")
}
