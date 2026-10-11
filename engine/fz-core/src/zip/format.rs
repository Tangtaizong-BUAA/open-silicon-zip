//! ZIP on-disk constants and small helpers shared by reader and writer.
use std::time::SystemTime;

pub const SIG_LOCAL: u32 = 0x04034b50;
pub const SIG_CENTRAL: u32 = 0x02014b50;
pub const SIG_EOCD: u32 = 0x06054b50;
pub const SIG_EOCD64: u32 = 0x06064b50;
pub const SIG_LOC64: u32 = 0x07064b50;
pub const SIG_DESC: u32 = 0x08074b50;
pub const SIG_SPAN: u32 = 0x08074b50;

pub const FLAG_ENCRYPTED: u16 = 1;
pub const FLAG_DESCRIPTOR: u16 = 1 << 3;
pub const FLAG_EFS: u16 = 1 << 11;

pub const M_STORE: u16 = 0;
pub const M_DEFLATE: u16 = 8;
pub const M_DEFLATE64: u16 = 9;
pub const M_BZIP2: u16 = 12;
pub const M_LZMA: u16 = 14;
pub const M_PPMD: u16 = 98;
pub const M_ZSTD: u16 = 93;
pub const M_AES: u16 = 99;

pub const EXTRA_ZIP64: u16 = 0x0001;
pub const EXTRA_NTFS: u16 = 0x000a;
pub const EXTRA_UT: u16 = 0x5455;
pub const EXTRA_UNICODE_PATH: u16 = 0x7075;
pub const EXTRA_AES: u16 = 0x9901;

pub const U32_MAX: u64 = 0xFFFF_FFFF;

/// Convert a unix timestamp to (dos_time, dos_date). Out-of-range (<1980, >2107) is clamped
/// to the nearest valid DOS value; the exact value still travels in the UT extra field.
pub fn dos_datetime(unix_secs: i64) -> (u16, u16, bool) {
    let (y, mo, d, h, mi, s) = civil_from_unix(unix_secs);
    let (mut clamped, mut y, mut mo, mut d, mut h, mut mi, mut s) = (false, y, mo, d, h, mi, s);
    if y < 1980 { clamped = true; y = 1980; mo = 1; d = 1; h = 0; mi = 0; s = 0; }
    if y > 2107 { clamped = true; y = 2107; mo = 12; d = 31; h = 23; mi = 59; s = 58; }
    let time = ((h << 11) | (mi << 5) | (s / 2)) as u16;
    let date = (((y - 1980) << 9) | (mo << 5) | d) as u16;
    (time, date, clamped)
}

pub fn dos_to_unix(time: u16, date: u16) -> i64 {
    let (s, mi, h) = ((time & 31) as i64 * 2, ((time >> 5) & 63) as i64, (time >> 11) as i64);
    let (d, mo, y) = ((date & 31) as i64, ((date >> 5) & 15) as i64, (date >> 9) as i64 + 1980);
    let (d, mo) = (d.max(1), mo.clamp(1, 12));
    days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + s
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_unix(t: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = t.div_euclid(86400);
    let rem = t.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

pub fn unix_secs(t: Option<SystemTime>) -> i64 {
    match t {
        Some(t) => match t.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => d.as_secs() as i64,
            Err(e) => -(e.duration().as_secs() as i64),
        },
        None => 0,
    }
}

/// Little-endian byte builder.
#[derive(Default)]
pub struct Le(pub Vec<u8>);
impl Le {
    pub fn u16(&mut self, v: u16) -> &mut Self { self.0.extend_from_slice(&v.to_le_bytes()); self }
    pub fn u32(&mut self, v: u32) -> &mut Self { self.0.extend_from_slice(&v.to_le_bytes()); self }
    pub fn u64(&mut self, v: u64) -> &mut Self { self.0.extend_from_slice(&v.to_le_bytes()); self }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self { self.0.extend_from_slice(b); self }
}

pub fn rd16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
pub fn rd32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }
pub fn rd64(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) }

/// Iterate extra-field records as (id, data).
pub fn extra_fields(mut e: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    while e.len() >= 4 {
        let id = rd16(e, 0);
        let len = rd16(e, 2) as usize;
        if 4 + len > e.len() { break; }
        out.push((id, &e[4..4 + len]));
        e = &e[4 + len..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dos_roundtrip() {
        let t = 1_700_000_000i64; // 2023-11-14 22:13:20
        let (tm, dt, c) = dos_datetime(t);
        assert!(!c);
        assert_eq!(dos_to_unix(tm, dt), t - (t % 2));
    }
    #[test]
    fn dos_clamps() {
        assert!(dos_datetime(0).2);
        assert_eq!(dos_datetime(0).1 >> 9, 0);
        assert!(dos_datetime(5_000_000_000_0).2);
    }
}
