//! Deterministic mutation fuzzer (smoke level).
//!
//! Builds small valid archives with the library (ZIP deflate/store/AES/ZipCrypto,
//! 7z plain/encrypted/header-encrypted, tar, tgz, tar.zst, gz, xz), then applies a
//! reproducible set of byte-level mutations to each one and feeds every mutant to the
//! public parsing APIs. Checks:
//!   * no panics (each call is wrapped in `catch_unwind`),
//!   * no hangs (each mutant runs in a worker thread with a 20 s deadline),
//!   * extraction never creates paths outside its own per-mutant directory.
//!
//! Every mutant is fully determined by `(archive, seed, index)`, so a failing recipe
//! id can be replayed with `reproduce()`. Known bugs are listed in `KNOWN_BUGS`; each
//! has a matching `#[ignore]` reproduction test.
use std::any::Any;
use std::collections::BTreeSet;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use fz_core::enumerate::{enumerate, EnumOptions};
use fz_core::formats::codecs::Codec;
use fz_core::formats::sevenz::{create_7z, SevenOptions, SzMethod, Solid};
use fz_core::formats::tar::{create_single_stream, create_tar, TarOptions};
use fz_core::formats::{self, detect};
use fz_core::zip::repair::repair_zip;
use fz_core::zip::{
    create_zip, test_archive, Encryption, ExtractOptions, Level, Method, Overwrite, ReadOptions,
    ZipArchive, ZipOptions,
};
use fz_core::{CancelToken, Progress};
use tempfile::TempDir;

const PW: &str = "correct horse battery";
const MUTANTS_PER_ARCHIVE: usize = 300;
const SEED_BASE: u64 = 0xF0_22_0000;
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_REPORTED: usize = 10;

/// Recipe ids (`"<archive>#<seed:#x>#<idx>"`) of confirmed bugs. Failures with these
/// ids do not fail the main test; each one has an `#[ignore]` reproduction below.
const KNOWN_BUGS: &[&str] = &[];

// ------------------------------------------------------------ fixtures

fn text(n: usize) -> Vec<u8> {
    b"The quick brown fox jumps over the lazy dog 0123456789\n".iter().cycle().take(n).copied().collect()
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

fn write(p: &Path, data: &[u8]) {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, data).unwrap();
}

/// Builds every fixture archive under `dir` and returns `(file name, bytes)` in a fixed order.
fn build_fixtures(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let src = dir.join("src");
    write(&src.join("a.txt"), &text(300));
    write(&src.join("sub/b.bin"), &noise(200, 3));
    fs::create_dir_all(src.join("empty")).unwrap();
    let cancel = CancelToken::new();
    let progress = Progress::new();
    let entries = enumerate(&[src.clone()], &EnumOptions::default(), &cancel).unwrap();
    let single = src.join("a.txt");
    let p = |n: &str| dir.join(n);

    let zip = |name: &str, o: ZipOptions| {
        create_zip(&p(name), &entries, &o, &cancel, &progress).unwrap();
    };
    zip("zip_deflate.zip", ZipOptions::default());
    zip("zip_store.zip", ZipOptions { method: Method::Store, ..Default::default() });
    zip(
        "zip_aes.zip",
        ZipOptions { encryption: Encryption::Aes { strength: 3, password: PW.into() }, ..Default::default() },
    );
    zip("zip_zipcrypto.zip", ZipOptions { encryption: Encryption::ZipCrypto(PW.into()), ..Default::default() });

    let sz = |name: &str, o: SevenOptions| {
        create_7z(&p(name), &entries, &o, &cancel, &progress).unwrap();
    };
    sz("sevenz_plain.7z", SevenOptions { method: SzMethod::Lzma2, solid: Solid::Bytes(1 << 20), ..Default::default() });
    sz("sevenz_enc.7z", SevenOptions { password: Some(PW.into()), ..Default::default() });
    sz("sevenz_hdr_enc.7z", SevenOptions { password: Some(PW.into()), encrypt_names: true, ..Default::default() });

    let tar = |name: &str, codec: Codec| {
        let o = TarOptions { codec, ..Default::default() };
        create_tar(&p(name), &entries, &o, &cancel, &progress).unwrap();
    };
    tar("bundle.tar", Codec::None);
    tar("bundle.tgz", Codec::Gzip);
    tar("bundle.tar.zst", Codec::Zstd);

    create_single_stream(&p("a.txt.gz"), &single, Codec::Gzip, Level::Normal, 0, &cancel, &progress).unwrap();
    create_single_stream(&p("a.txt.xz"), &single, Codec::Xz, Level::Normal, 0, &cancel, &progress).unwrap();

    [
        "zip_deflate.zip",
        "zip_store.zip",
        "zip_aes.zip",
        "zip_zipcrypto.zip",
        "sevenz_plain.7z",
        "sevenz_enc.7z",
        "sevenz_hdr_enc.7z",
        "bundle.tar",
        "bundle.tgz",
        "bundle.tar.zst",
        "a.txt.gz",
        "a.txt.xz",
    ]
    .iter()
    .map(|n| (n.to_string(), fs::read(p(n)).unwrap()))
    .collect()
}

// ------------------------------------------------------------ mutations

/// Small xorshift-based RNG (splitmix-seeded). Only used for mutation choice.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Rng(z | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Uniform-ish value in `0..n` (0 when `n == 0`).
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }
    fn byte(&mut self) -> u8 {
        (self.next_u64() >> 24) as u8
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Mutation {
    Flip { pos: usize, mask: u8 },
    Multi(Vec<(usize, u8)>),
    Truncate(usize),
    ZeroWindow { pos: usize, len: usize },
    Insert { pos: usize, data: Vec<u8> },
    Delete { pos: usize, len: usize },
    /// Little-endian overwrite of `width` bytes at `pos`.
    Field { pos: usize, width: usize, value: u64 },
}

impl Mutation {
    fn apply(&self, base: &[u8]) -> Vec<u8> {
        let mut v = base.to_vec();
        let len = v.len();
        match self {
            Mutation::Flip { pos, mask } => {
                if *pos < len {
                    v[*pos] ^= mask;
                }
            }
            Mutation::Multi(list) => {
                for (p, m) in list {
                    if *p < len {
                        v[*p] ^= m;
                    }
                }
            }
            Mutation::Truncate(n) => v.truncate((*n).min(len)),
            Mutation::ZeroWindow { pos, len: l } => {
                for b in v.iter_mut().skip(*pos).take(*l) {
                    *b = 0;
                }
            }
            Mutation::Insert { pos, data } => {
                let p = (*pos).min(len);
                v.splice(p..p, data.iter().copied());
            }
            Mutation::Delete { pos, len: l } => {
                let p = (*pos).min(len);
                let e = p.saturating_add(*l).min(len);
                v.drain(p..e);
            }
            Mutation::Field { pos, width, value } => {
                let le = value.to_le_bytes();
                for i in 0..*width {
                    if pos + i < len {
                        v[pos + i] = le[i];
                    }
                }
            }
        }
        v
    }
}

/// Field position: near the head (first 64 bytes), near the tail (last 256 bytes,
/// where end-of-central-directory / 7z end header / zip64 locators live), or anywhere.
fn field_pos(rng: &mut Rng, len: usize, width: usize) -> usize {
    let max = len.saturating_sub(width).max(0);
    let p = match rng.below(3) {
        0 => rng.below(64.min(max + 1)),
        1 => max.saturating_sub(255) + rng.below(256.min(max + 1)),
        _ => rng.below(max + 1),
    };
    p.min(max)
}

fn generate(rng: &mut Rng, base: &[u8]) -> Mutation {
    let len = base.len().max(1);
    match rng.below(8) {
        0 => Mutation::Flip { pos: rng.below(len), mask: 1 + rng.below(255) as u8 },
        1 => {
            let k = 2 + rng.below(15);
            let list = (0..k).map(|_| (rng.below(len), 1 + rng.below(255) as u8)).collect();
            Mutation::Multi(list)
        }
        2 => Mutation::Truncate(rng.below(len)),
        3 => Mutation::ZeroWindow { pos: rng.below(len), len: 16 },
        4 => {
            let n = 1 + rng.below(32);
            let data = (0..n).map(|_| rng.byte()).collect();
            Mutation::Insert { pos: rng.below(base.len() + 1), data }
        }
        5 => Mutation::Delete { pos: rng.below(len), len: 1 + rng.below(32) },
        6 => {
            const V: [u64; 6] = [0xFFFF_FFFF, 0, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFF0, 0x0001_0000];
            Mutation::Field { pos: field_pos(rng, base.len(), 4), width: 4, value: V[rng.below(V.len())] }
        }
        _ => {
            const V: [u64; 5] = [0xFFFF, 0, 0x8000, 0x7FFF, 0xFFF0];
            Mutation::Field { pos: field_pos(rng, base.len(), 2), width: 2, value: V[rng.below(V.len())] }
        }
    }
}

/// The mutation for `(base, seed, idx)`. Pure function of its inputs.
fn mutant_for(base: &[u8], seed: u64, idx: usize) -> Mutation {
    let mut rng = Rng::new(seed ^ (idx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    generate(&mut rng, base)
}

fn recipe_id(name: &str, seed: u64, idx: usize) -> String {
    format!("{name}#{seed:#x}#{idx}")
}

/// Bytes of the mutant identified by `(name, seed, idx)` plus a human description.
/// `fixtures` is the output of `build_fixtures`.
#[allow(dead_code)] // used by the reproduction tests for confirmed bugs
fn reproduce(fixtures: &[(String, Vec<u8>)], name: &str, seed: u64, idx: usize) -> (Mutation, Vec<u8>) {
    let (_, base) = fixtures.iter().find(|(n, _)| n == name).expect("unknown archive name");
    let m = mutant_for(base, seed, idx);
    let bytes = m.apply(base);
    (m, bytes)
}

// ------------------------------------------------------------ execution

#[derive(Debug)]
struct Failure {
    recipe: String,
    mutation: String,
    what: String,
}

struct Job {
    input: PathBuf,
    dir: PathBuf,
    is_zip: bool,
}

fn payload_str(e: &(dyn Any + Send)) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

macro_rules! guard {
    ($panics:ident, $label:expr, $body:expr) => {
        if let Err(e) = panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = $body;
        })) {
            $panics.push(($label.to_string(), payload_str(&*e)));
        }
    };
}

/// Runs every API on the mutant. Runs on a worker thread. Returns the panics seen.
fn worker(job: Job) -> Vec<(String, String)> {
    let mut panics: Vec<(String, String)> = Vec::new();
    let input = job.input.as_path();
    let cancel = CancelToken::new();
    let progress = Progress::new();
    let pw = Some(PW);

    guard!(panics, "detect", detect(input));
    guard!(panics, "list(no pw)", formats::list(input, None, None, &cancel));
    guard!(panics, "list(pw)", formats::list(input, pw, None, &cancel));
    guard!(panics, "test(no pw)", formats::test(input, None, &cancel, &progress));
    guard!(panics, "test(pw)", formats::test(input, pw, &cancel, &progress));

    for (label, password, sub) in [("extract(no pw)", None, "out_a"), ("extract(pw)", pw, "out_b")] {
        let mut o = ExtractOptions::new(job.dir.join(sub));
        o.smart = true;
        o.overwrite = Overwrite::Rename;
        o.password = password.map(str::to_string);
        guard!(panics, label, formats::extract(input, &o, None, &cancel, &progress));
    }

    if job.is_zip {
        guard!(panics, "ZipArchive::open+test_archive", {
            match ZipArchive::open(input, ReadOptions::default()) {
                Ok(arc) => {
                    let _ = test_archive(&arc, None, &cancel, &progress);
                    let _ = test_archive(&arc, pw, &cancel, &progress);
                }
                Err(_) => {}
            }
        });
        guard!(panics, "repair_zip", repair_zip(input, &job.dir.join("repaired.zip"), &cancel));
    }
    panics
}

/// Recursive listing of `base` (no symlink following), skipping `leaked` subtrees.
fn snapshot(base: &Path, leaked: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut set = BTreeSet::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if leaked.iter().any(|l| p.starts_with(l)) {
                continue;
            }
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(p.clone());
            }
            set.insert(p);
        }
    }
    set
}

/// Runs one mutant end to end and returns its failures.
/// `outer` contains only `mutants/`; a timed-out mutant's directory is moved to `leaked`
/// so that its stray writes do not pollute later escape checks.
fn check_mutant(
    outer: &Path,
    name: &str,
    recipe: &str,
    desc: &str,
    bytes: &[u8],
    leaked: &mut Vec<PathBuf>,
) -> Vec<Failure> {
    let dir = outer.join("mutants").join(recipe.replace(['#', '/', ' '], "_"));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join(name);
    fs::write(&input, bytes).unwrap();
    let before = snapshot(outer, leaked);

    let job = Job { input: input.clone(), dir: dir.clone(), is_zip: name.ends_with(".zip") };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(worker(job));
    });
    let fail = |what: String| Failure { recipe: recipe.to_string(), mutation: desc.to_string(), what };

    match rx.recv_timeout(TIMEOUT) {
        Ok(panics) => {
            let mut out: Vec<Failure> = panics.into_iter().map(|(call, msg)| fail(format!("panic in {call}: {msg}"))).collect();
            let after = snapshot(outer, leaked);
            for p in after.difference(&before) {
                if !p.starts_with(&dir) {
                    out.push(fail(format!("escape: created {}", p.display())));
                }
            }
            let _ = fs::remove_dir_all(&dir);
            out
        }
        Err(RecvTimeoutError::Timeout) => {
            leaked.push(dir);
            vec![fail(format!("timeout: no return within {}s", TIMEOUT.as_secs()))]
        }
        Err(RecvTimeoutError::Disconnected) => {
            leaked.push(dir);
            vec![fail("worker thread ended without reporting".to_string())]
        }
    }
}

// ------------------------------------------------------------ tests

#[test]
fn fuzz_smoke_mutants_never_panic_hang_or_escape() {
    let fx = TempDir::new().unwrap();
    let fixtures = build_fixtures(fx.path());
    let outer = TempDir::new().unwrap();
    fs::create_dir_all(outer.path().join("mutants")).unwrap();

    let mut leaked: Vec<PathBuf> = Vec::new();
    let mut failures: Vec<Failure> = Vec::new();
    let mut ran = 0usize;
    let mut known_hits = 0usize;

    for (ai, (name, base)) in fixtures.iter().enumerate() {
        let seed = SEED_BASE + ai as u64;
        for idx in 0..MUTANTS_PER_ARCHIVE {
            let m = mutant_for(base, seed, idx);
            let bytes = m.apply(base);
            let recipe = recipe_id(name, seed, idx);
            ran += 1;
            for f in check_mutant(outer.path(), name, &recipe, &format!("{m:?}"), &bytes, &mut leaked) {
                if KNOWN_BUGS.contains(&f.recipe.as_str()) {
                    known_hits += 1;
                } else {
                    failures.push(f);
                }
            }
        }
    }

    eprintln!(
        "fuzz_smoke: {ran} mutants over {} archives; {} unexpected failures; {known_hits} known-bug hits; {} timed-out dirs leaked",
        fixtures.len(),
        failures.len(),
        leaked.len()
    );
    if !failures.is_empty() {
        let mut msg = format!("{} unexpected failure(s) out of {ran} mutants; first {}:\n", failures.len(), MAX_REPORTED);
        for f in failures.iter().take(MAX_REPORTED) {
            msg.push_str(&format!("  - recipe {}\n      mutation {}\n      {}\n", f.recipe, f.mutation, f.what));
        }
        panic!("{msg}");
    }
}

// Reproduction of confirmed bugs (none yet). Template:
//
// #[test]
// #[ignore = "BUG: <file:function> ..."]
// fn bug_<short>() {
//     let fx = TempDir::new().unwrap();
//     let fixtures = build_fixtures(fx.path());
//     let (m, bytes) = reproduce(&fixtures, "<archive>", <seed>, <idx>);
//     let outer = TempDir::new().unwrap();
//     fs::create_dir_all(outer.path().join("mutants")).unwrap();
//     let f = check_mutant(outer.path(), "<archive>", "<recipe>", &format!("{m:?}"), &bytes, &mut Vec::new());
//     assert!(f.is_empty(), "{f:?}");
// }
