//! FR-703: brute-force password recovery for ZIP archives (ZipCrypto and AES).
//!
//! The search space for each total length `L` is `charset^(L - prefix_len)`
//! candidates, enumerated by mixed-radix index. Worker threads claim chunks of
//! [`CHUNK`] indices from a shared cursor; a cheap [`PasswordVerifier::check`]
//! runs first and the full [`ZipArchive::confirm_password`] only on a pass.
//!
//! Cancellation returns a conservative [`Checkpoint`]: the lowest chunk that was
//! claimed but not fully tested. Resuming may redo some work but never skips any.
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::cancel::CancelToken;
use crate::error::{Error, ErrorKind, Result};
use crate::sys::perf_cores;
use crate::zip::{PasswordVerifier, ReadOptions, ZipArchive};

/// Candidates claimed per chunk.
const CHUNK: u128 = 4096;
const MAX_LEN_LIMIT: usize = 12;

// ------------------------------------------------------------------ charset

/// Ordered, de-duplicated set of candidate characters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Charset {
    pub chars: Vec<char>,
}

impl Charset {
    pub fn digits() -> Self {
        Self::custom("0123456789")
    }

    pub fn lower() -> Self {
        Self::custom("abcdefghijklmnopqrstuvwxyz")
    }

    pub fn upper() -> Self {
        Self::custom("ABCDEFGHIJKLMNOPQRSTUVWXYZ")
    }

    /// The printable ASCII punctuation characters.
    pub fn special() -> Self {
        Self { chars: (0u8..128).map(char::from).filter(|c| c.is_ascii_punctuation()).collect() }
    }

    pub fn space() -> Self {
        Self::custom(" ")
    }

    /// Lower, upper, digits, special and space.
    pub fn all() -> Self {
        Self::lower()
            .merge(&Self::upper())
            .merge(&Self::digits())
            .merge(&Self::special())
            .merge(&Self::space())
    }

    /// Characters of `s` in order, duplicates removed.
    pub fn custom(s: &str) -> Self {
        let mut chars = Vec::new();
        for c in s.chars() {
            if !chars.contains(&c) {
                chars.push(c);
            }
        }
        Self { chars }
    }

    /// Union with `other`, keeping this charset's order first, duplicates removed.
    pub fn merge(&self, other: &Charset) -> Self {
        let mut chars = self.chars.clone();
        for &c in &other.chars {
            if !chars.contains(&c) {
                chars.push(c);
            }
        }
        Self { chars }
    }
}

// ------------------------------------------------------------- configuration

/// A resume point: total candidate length and the mixed-radix index within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub len: usize,
    pub index: u128,
}

impl Checkpoint {
    pub fn to_json(&self) -> String {
        serde_json::json!({ "len": self.len, "index": self.index.to_string() }).to_string()
    }

    pub fn from_json(s: &str) -> Result<Self> {
        let bad = |why: &str| Error::new(ErrorKind::InvalidInput, format!("invalid checkpoint: {why}"));
        let v: serde_json::Value = serde_json::from_str(s).map_err(|e| bad(&e.to_string()))?;
        let len = v.get("len").and_then(|x| x.as_u64()).ok_or_else(|| bad("missing len"))?;
        let index = v
            .get("index")
            .and_then(|x| x.as_str())
            .and_then(|x| x.parse::<u128>().ok())
            .ok_or_else(|| bad("missing or non-numeric index"))?;
        Ok(Checkpoint { len: len as usize, index })
    }
}

#[derive(Clone, Debug)]
pub struct RecoverConfig {
    pub charset: Charset,
    /// Fixed leading characters; counted toward the total length.
    pub prefix: String,
    pub min_len: usize,
    pub max_len: usize,
    /// 0 = auto (performance cores).
    pub threads: usize,
    pub resume: Option<Checkpoint>,
}

/// Cooperative pause flag shared with the workers.
#[derive(Clone, Debug, Default)]
pub struct PauseToken(Arc<AtomicBool>);

impl PauseToken {
    pub fn pause(&self) {
        self.0.store(true, Relaxed)
    }
    pub fn resume(&self) {
        self.0.store(false, Relaxed)
    }
    pub fn is_paused(&self) -> bool {
        self.0.load(Relaxed)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatsSnapshot {
    pub tested: u64,
    pub per_second: f64,
    pub elapsed_secs: f64,
    pub eta_secs: Option<f64>,
    pub current_len: usize,
}

#[derive(Clone, Debug, Default)]
pub struct RecoverStats(Arc<StatsInner>);

#[derive(Debug, Default)]
struct StatsInner {
    tested: AtomicU64,
    current_len: AtomicUsize,
    total: Mutex<u128>,
    started: Mutex<Option<Instant>>,
}

impl RecoverStats {
    fn begin(&self, total: u128) {
        let i = &self.0;
        i.tested.store(0, Relaxed);
        i.current_len.store(0, Relaxed);
        *i.total.lock().unwrap() = total;
        *i.started.lock().unwrap() = Some(Instant::now());
    }

    fn add_tested(&self, n: u64) {
        self.0.tested.fetch_add(n, Relaxed);
    }

    fn set_len(&self, len: usize) {
        self.0.current_len.store(len, Relaxed);
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        let i = &self.0;
        let tested = i.tested.load(Relaxed);
        let elapsed_secs = i.started.lock().unwrap().map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
        let per_second = if elapsed_secs > 0.0 { tested as f64 / elapsed_secs } else { 0.0 };
        let total = *i.total.lock().unwrap();
        let remaining = total.saturating_sub(tested as u128);
        let eta_secs = if per_second > 0.0 { Some(remaining as f64 / per_second) } else { None };
        StatsSnapshot { tested, per_second, elapsed_secs, eta_secs, current_len: i.current_len.load(Relaxed) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Found(String),
    Exhausted,
    Cancelled(Checkpoint),
}

// ------------------------------------------------------------------ planning

fn invalid(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidInput, msg)
}

fn validate(cfg: &RecoverConfig) -> Result<()> {
    if !(1..=MAX_LEN_LIMIT).contains(&cfg.min_len) || !(1..=MAX_LEN_LIMIT).contains(&cfg.max_len) {
        return Err(invalid("min_len and max_len must be within 1..=12"));
    }
    if cfg.min_len > cfg.max_len {
        return Err(invalid("min_len must not exceed max_len"));
    }
    if cfg.charset.chars.is_empty() {
        return Err(invalid("charset is empty"));
    }
    Ok(())
}

/// One total length to search. `start` is the first index still to test.
#[derive(Debug, Clone, Copy)]
struct Seg {
    len: usize,
    free: usize,
    start: u128,
    space: u128,
}

fn build_plan(cfg: &RecoverConfig, resume: Option<&Checkpoint>) -> Result<Vec<Seg>> {
    validate(cfg)?;
    let n = cfg.charset.chars.len() as u128;
    let plen = cfg.prefix.chars().count();
    if let Some(r) = resume {
        if r.len < cfg.min_len || r.len > cfg.max_len || r.len < plen {
            return Err(invalid("checkpoint length is outside the configured range"));
        }
    }
    let mut plan = Vec::new();
    for len in cfg.min_len.max(plen)..=cfg.max_len {
        if let Some(r) = resume {
            if r.len > len {
                continue;
            }
        }
        let free = len - plen;
        let space = n
            .checked_pow(free as u32)
            .ok_or_else(|| invalid("search space overflows u128"))?;
        let start = match resume {
            Some(r) if r.len == len => {
                if r.index >= space {
                    return Err(invalid("checkpoint index is outside the search space"));
                }
                r.index
            }
            _ => 0,
        };
        plan.push(Seg { len, free, start, space });
    }
    Ok(plan)
}

/// Total number of candidates for `cfg` (ignoring any resume point), or `None`
/// if the configuration is invalid or the count overflows `u128`.
pub fn estimate_total(cfg: &RecoverConfig) -> Option<u128> {
    let plan = build_plan(cfg, None).ok()?;
    plan.iter().try_fold(0u128, |acc, s| acc.checked_add(s.space))
}

/// Builds the candidate for free-part index `idx`: prefix followed by `free`
/// characters, most significant first, in mixed radix over `chars`.
fn candidate(prefix: &str, chars: &[char], free: usize, mut idx: u128) -> String {
    let n = chars.len() as u128;
    let mut tail = vec![' '; free];
    for slot in tail.iter_mut().rev() {
        *slot = chars[(idx % n) as usize];
        idx /= n;
    }
    let mut s = String::with_capacity(prefix.len() + free * 4);
    s.push_str(prefix);
    s.extend(tail);
    s
}

// ------------------------------------------------------------------ search

struct Cursor {
    /// Index into the plan of the segment currently being claimed.
    idx: usize,
    /// Next unclaimed index within that segment.
    next: u128,
    /// Claimed but not fully tested chunks, as (plan index, chunk start).
    in_flight: BTreeSet<(usize, u128)>,
}

struct Shared<'a> {
    plan: &'a [Seg],
    cursor: Mutex<Cursor>,
    found: Mutex<Option<String>>,
    stop: AtomicBool,
}

impl<'a> Shared<'a> {
    fn new(plan: &'a [Seg]) -> Self {
        let next = plan.first().map(|s| s.start).unwrap_or(0);
        Shared {
            plan,
            cursor: Mutex::new(Cursor { idx: 0, next, in_flight: BTreeSet::new() }),
            found: Mutex::new(None),
            stop: AtomicBool::new(false),
        }
    }

    /// Claims the next chunk as (plan index, from, to).
    fn claim(&self) -> Option<(usize, u128, u128)> {
        let mut c = self.cursor.lock().unwrap();
        loop {
            let seg = *self.plan.get(c.idx)?;
            if c.next >= seg.space {
                c.idx += 1;
                if let Some(nseg) = self.plan.get(c.idx) {
                    c.next = nseg.start;
                }
                continue;
            }
            let from = c.next;
            let to = from.saturating_add(CHUNK).min(seg.space);
            c.next = to;
            let idx = c.idx;
            c.in_flight.insert((idx, from));
            return Some((idx, from, to));
        }
    }

    fn complete(&self, pi: usize, from: u128) {
        self.cursor.lock().unwrap().in_flight.remove(&(pi, from));
    }

    /// Lowest untested position: the first in-flight chunk, else the cursor.
    fn resume_point(&self) -> Option<Checkpoint> {
        let c = self.cursor.lock().unwrap();
        if let Some(&(pi, start)) = c.in_flight.iter().next() {
            return Some(Checkpoint { len: self.plan[pi].len, index: start });
        }
        let mut idx = c.idx;
        let mut next = c.next;
        while let Some(seg) = self.plan.get(idx) {
            if next < seg.space {
                return Some(Checkpoint { len: seg.len, index: next });
            }
            idx += 1;
            next = self.plan.get(idx).map(|s| s.start).unwrap_or(0);
        }
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn worker(
    sh: &Shared,
    archive: &ZipArchive,
    verifier: &PasswordVerifier,
    cfg: &RecoverConfig,
    cancel: &CancelToken,
    pause: &PauseToken,
    stats: &RecoverStats,
) {
    let chars = &cfg.charset.chars;
    while let Some((pi, from, to)) = sh.claim() {
        let seg = sh.plan[pi];
        stats.set_len(seg.len);
        let mut tested = 0u64;
        let mut idx = from;
        let mut interrupted = false;
        while idx < to {
            // Gate: honour pause (sleeping) and stop/cancel (exit) before each candidate.
            loop {
                if cancel.is_cancelled() || sh.stop.load(Relaxed) {
                    interrupted = true;
                    break;
                }
                if !pause.is_paused() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            if interrupted {
                break;
            }
            let pw = candidate(&cfg.prefix, chars, seg.free, idx);
            if verifier.check(pw.as_bytes()) && archive.confirm_password(verifier, &pw) {
                stats.add_tested(tested + 1);
                *sh.found.lock().unwrap() = Some(pw);
                sh.stop.store(true, Relaxed);
                return;
            }
            tested += 1;
            idx += 1;
        }
        stats.add_tested(tested);
        if interrupted {
            // Chunk left in flight so a resume re-tests it.
            return;
        }
        sh.complete(pi, from);
    }
}

/// Searches `path` for a password matching `cfg`. Blocks until found, exhausted
/// or cancelled. Returns `Err(InvalidInput)` for a bad configuration, and an
/// error if the archive cannot be opened or is not encrypted.
pub fn recover(
    path: &Path,
    cfg: &RecoverConfig,
    cancel: &CancelToken,
    pause: &PauseToken,
    stats: &RecoverStats,
) -> Result<Outcome> {
    let plan = build_plan(cfg, cfg.resume.as_ref())?;
    let archive = ZipArchive::open(path, ReadOptions::default())?;
    let verifier = archive.verifier()?;

    let total: u128 = plan.iter().map(|s| s.space - s.start).sum();
    stats.begin(total);

    let threads = if cfg.threads == 0 { perf_cores() } else { cfg.threads }.max(1);
    let sh = Shared::new(&plan);
    if !plan.is_empty() {
        thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| worker(&sh, &archive, &verifier, cfg, cancel, pause, stats));
            }
        });
    }

    if let Some(pw) = sh.found.lock().unwrap().take() {
        return Ok(Outcome::Found(pw));
    }
    if cancel.is_cancelled() {
        if let Some(cp) = sh.resume_point() {
            return Ok(Outcome::Cancelled(cp));
        }
    }
    Ok(Outcome::Exhausted)
}

#[allow(dead_code)]
fn _assert_sync() {
    fn is_sync<T: Sync>() {}
    is_sync::<ZipArchive>();
    is_sync::<PasswordVerifier>();
}

// ------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::Progress;
    use crate::enumerate::{enumerate, EnumOptions};
    use crate::zip::{create_zip, Encryption, ZipOptions};
    use std::fs;
    use tempfile::TempDir;

    fn make_zip(enc: Encryption) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("secret.txt");
        fs::write(&src, b"hello recovery").unwrap();
        let zip = dir.path().join("out.zip");
        let entries = enumerate(&[src], &EnumOptions::default(), &CancelToken::new()).unwrap();
        let opts = ZipOptions { encryption: enc, ..Default::default() };
        create_zip(&zip, &entries, &opts, &CancelToken::new(), &Progress::new()).unwrap();
        (dir, zip)
    }

    fn cfg(charset: Charset, prefix: &str, min: usize, max: usize) -> RecoverConfig {
        RecoverConfig { charset, prefix: prefix.into(), min_len: min, max_len: max, threads: 2, resume: None }
    }

    fn run(zip: &Path, c: &RecoverConfig, cancel: &CancelToken) -> Outcome {
        recover(zip, c, cancel, &PauseToken::default(), &RecoverStats::default()).unwrap()
    }

    fn lower_digits() -> Charset {
        Charset::lower().merge(&Charset::digits())
    }

    #[test]
    fn recover_zipcrypto_finds_password() {
        let (_d, zip) = make_zip(Encryption::ZipCrypto("ab1".into()));
        let out = run(&zip, &cfg(lower_digits(), "", 1, 3), &CancelToken::new());
        assert_eq!(out, Outcome::Found("ab1".into()));
    }

    #[test]
    fn recover_aes_finds_password() {
        let (_d, zip) = make_zip(Encryption::Aes { strength: 3, password: "42".into() });
        let out = run(&zip, &cfg(Charset::digits(), "", 1, 2), &CancelToken::new());
        assert_eq!(out, Outcome::Found("42".into()));
    }

    #[test]
    fn recover_wrong_charset_is_exhausted() {
        let (_d, zip) = make_zip(Encryption::ZipCrypto("ab1".into()));
        let out = run(&zip, &cfg(Charset::digits(), "", 1, 3), &CancelToken::new());
        assert_eq!(out, Outcome::Exhausted);
    }

    #[test]
    fn recover_prefix_plus_digits_finds_password() {
        let (_d, zip) = make_zip(Encryption::ZipCrypto("ab1".into()));
        let out = run(&zip, &cfg(Charset::digits(), "ab", 3, 3), &CancelToken::new());
        assert_eq!(out, Outcome::Found("ab1".into()));
    }

    #[test]
    fn recover_precancelled_then_resume_finds_password() {
        let (_d, zip) = make_zip(Encryption::ZipCrypto("ab1".into()));
        let c = cfg(lower_digits(), "", 1, 3);
        let cancel = CancelToken::new();
        cancel.cancel();
        let cp = match run(&zip, &c, &cancel) {
            Outcome::Cancelled(cp) => cp,
            other => panic!("expected Cancelled, got {other:?}"),
        };
        let resumed = RecoverConfig { resume: Some(cp), ..c };
        assert_eq!(run(&zip, &resumed, &CancelToken::new()), Outcome::Found("ab1".into()));
    }

    #[test]
    fn recover_cancel_from_thread_then_resume_finds_password() {
        let (_d, zip) = make_zip(Encryption::ZipCrypto("zzz".into()));
        let c = cfg(lower_digits(), "", 3, 3);
        let cancel = CancelToken::new();
        let out = thread::scope(|s| {
            let c2 = cancel.clone();
            s.spawn(move || {
                thread::sleep(Duration::from_millis(1));
                c2.cancel();
            });
            run(&zip, &c, &cancel)
        });
        match out {
            Outcome::Found(pw) => assert_eq!(pw, "zzz"),
            Outcome::Cancelled(cp) => {
                let resumed = RecoverConfig { resume: Some(cp), ..c };
                assert_eq!(run(&zip, &resumed, &CancelToken::new()), Outcome::Found("zzz".into()));
            }
            Outcome::Exhausted => panic!("password space was not exhausted without cancel"),
        }
    }

    #[test]
    fn recover_stats_count_tested_candidates() {
        let (_d, zip) = make_zip(Encryption::ZipCrypto("ab1".into()));
        let stats = RecoverStats::default();
        let out = recover(&zip, &cfg(Charset::digits(), "", 2, 2), &CancelToken::new(), &PauseToken::default(), &stats).unwrap();
        assert_eq!(out, Outcome::Exhausted);
        assert_eq!(stats.snapshot().tested, 100);
    }

    #[test]
    fn recover_rejects_bad_length_range() {
        let c = cfg(Charset::digits(), "", 1, 13);
        let err = recover(Path::new("/nonexistent.zip"), &c, &CancelToken::new(), &PauseToken::default(), &RecoverStats::default())
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        let c = cfg(Charset::digits(), "", 0, 3);
        assert!(estimate_total(&c).is_none());
        let c = cfg(Charset::digits(), "", 4, 3);
        assert!(estimate_total(&c).is_none());
    }

    #[test]
    fn recover_rejects_out_of_range_resume() {
        let c = RecoverConfig { resume: Some(Checkpoint { len: 5, index: 0 }), ..cfg(Charset::digits(), "", 1, 3) };
        let err = recover(Path::new("/nonexistent.zip"), &c, &CancelToken::new(), &PauseToken::default(), &RecoverStats::default())
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
    }

    #[test]
    fn checkpoint_json_roundtrip() {
        let cp = Checkpoint { len: 7, index: u128::MAX - 5 };
        let json = cp.to_json();
        assert!(json.contains("\"index\":\""), "index must be a JSON string: {json}");
        assert_eq!(Checkpoint::from_json(&json).unwrap(), cp);
        let zero = Checkpoint { len: 1, index: 0 };
        assert_eq!(Checkpoint::from_json(&zero.to_json()).unwrap(), zero);
        assert_eq!(Checkpoint::from_json("nope").unwrap_err().kind, ErrorKind::InvalidInput);
        assert_eq!(Checkpoint::from_json("{\"len\":2,\"index\":\"x\"}").unwrap_err().kind, ErrorKind::InvalidInput);
    }

    #[test]
    fn estimate_total_counts_prefix_and_lengths() {
        assert_eq!(estimate_total(&cfg(Charset::digits(), "", 3, 3)), Some(1000));
        // prefix "ab" (2 chars): L=2 tests "ab" alone, L=3 tests 10 candidates.
        assert_eq!(estimate_total(&cfg(Charset::digits(), "ab", 1, 3)), Some(11));
        assert_eq!(estimate_total(&cfg(Charset::digits(), "", 1, 2)), Some(110));
    }

    #[test]
    fn candidate_mixed_radix_order() {
        let ab = ['a', 'b'];
        assert_eq!(candidate("x", &ab, 2, 0), "xaa");
        assert_eq!(candidate("x", &ab, 2, 1), "xab");
        assert_eq!(candidate("x", &ab, 2, 2), "xba");
        assert_eq!(candidate("x", &ab, 2, 3), "xbb");
        assert_eq!(candidate("x", &ab, 0, 0), "x");
    }

    #[test]
    fn charset_constructors_and_merge() {
        assert_eq!(Charset::custom("aab").merge(&Charset::custom("bcc")).chars, vec!['a', 'b', 'c']);
        assert_eq!(Charset::digits().chars.len(), 10);
        assert_eq!(Charset::all().chars.len(), 26 + 26 + 10 + 32 + 1);
        let mut all = Charset::all().chars;
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n, "all() must not contain duplicates");
    }
}
