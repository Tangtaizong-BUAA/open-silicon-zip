//! Checksums (FR-704): CRC32, CRC64 (ECMA-182), MD5, SHA-1, SHA-256, computed in one pass.
use crate::error::Result;
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::io::Read;

pub struct Crc64 { table: [u64; 256], v: u64 }
impl Crc64 {
    pub fn new() -> Self {
        let poly: u64 = 0xC96C5795D7870F42; // reflected ECMA-182 (xz)
        let mut table = [0u64; 256];
        for i in 0..256u64 {
            let mut c = i;
            for _ in 0..8 { c = if c & 1 == 1 { (c >> 1) ^ poly } else { c >> 1 }; }
            table[i as usize] = c;
        }
        Crc64 { table, v: !0 }
    }
    pub fn update(&mut self, d: &[u8]) {
        for &b in d { self.v = self.table[((self.v ^ b as u64) & 0xff) as usize] ^ (self.v >> 8); }
    }
    pub fn finish(&self) -> u64 { !self.v }
}

#[derive(Debug, Clone)]
pub struct Digests {
    pub crc32: String,
    pub crc64: String,
    pub md5: String,
    pub sha1: String,
    pub sha256: String,
    pub size: u64,
}

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

pub fn digest_reader<R: Read>(mut r: R) -> Result<Digests> {
    let (mut c32, mut c64) = (crc32fast::Hasher::new(), Crc64::new());
    let (mut m, mut s1, mut s256) = (Md5::new(), Sha1::new(), Sha256::new());
    let mut buf = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 { break; }
        let d = &buf[..n];
        size += n as u64;
        c32.update(d); c64.update(d); m.update(d); s1.update(d); s256.update(d);
    }
    Ok(Digests {
        crc32: format!("{:08x}", c32.finalize()),
        crc64: format!("{:016x}", c64.finish()),
        md5: hex(&m.finalize()), sha1: hex(&s1.finalize()), sha256: hex(&s256.finalize()), size,
    })
}

pub fn digest_file(p: &std::path::Path) -> Result<Digests> { digest_reader(std::fs::File::open(p)?) }

/// Compare an expected pasted value: Full / Partial (prefix or suffix, >= 6 hex chars) / Mismatch.
#[derive(Debug, PartialEq, Eq)]
pub enum MatchKind { Full, Partial, Mismatch }
pub fn compare_expected(d: &Digests, expected: &str) -> MatchKind {
    let e = expected.trim().to_ascii_lowercase();
    if e.is_empty() { return MatchKind::Mismatch; }
    let all = [&d.crc32, &d.crc64, &d.md5, &d.sha1, &d.sha256];
    if all.iter().any(|h| **h == e) { return MatchKind::Full; }
    if e.len() >= 6 && all.iter().any(|h| h.starts_with(&e) || h.ends_with(&e)) { return MatchKind::Partial; }
    MatchKind::Mismatch
}
