//! ZIP repair (FR-702/706): streaming central-directory reconstruction + partial salvage.
//! Never modifies the input; output is always a new file; memory use is bounded (streaming).
use super::format::*;
use crate::cancel::CancelToken;
use crate::error::{Error, ErrorKind, Result};
use flate2::{Decompress, FlushDecompress, Status};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

#[derive(Debug, Default)]
pub struct RepairReport {
    pub recovered: Vec<String>,
    /// Entries whose data was damaged; the readable prefix was saved as `<name>.recovered`.
    pub partial: Vec<(String, String)>,
    pub lost: Vec<(String, String)>,
    pub strategy: &'static str,
}

fn find_sig(f: &File, mut from: u64, len: u64, sigs: &[[u8; 4]]) -> Result<Option<(u64, [u8; 4])>> {
    let mut buf = vec![0u8; 1 << 20];
    while from + 4 <= len {
        let n = ((len - from) as usize).min(buf.len());
        f.read_exact_at(&mut buf[..n], from)?;
        for i in 0..n.saturating_sub(3) {
            let w = [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]];
            if sigs.contains(&w) { return Ok(Some((from + i as u64, w))); }
        }
        if n < 4 { break; }
        from += (n - 3) as u64;
    }
    Ok(None)
}

struct Rec { hdr_flags: u16, method: u16, time: u16, date: u16, crc: u32, csize: u64, usize: u64, name: Vec<u8>, extra: Vec<u8>, off: u64 }

fn push_central(cd: &mut Vec<u8>, r: &Rec) {
    let is_dir = r.name.last() == Some(&b'/');
    let mut b = Le::default();
    b.u32(SIG_CENTRAL).u16((3 << 8) | 63).u16(if r.method == M_AES { 51 } else { 20 }).u16(r.hdr_flags).u16(r.method).u16(r.time).u16(r.date)
        .u32(r.crc).u32(r.csize as u32).u32(r.usize as u32).u16(r.name.len() as u16).u16(r.extra.len() as u16).u16(0).u16(0).u16(0)
        .u32(if is_dir { 0x10 | (0o040755 << 16) } else { 0x20 | (0o100644 << 16) }).u32(r.off as u32).bytes(&r.name).bytes(&r.extra);
    cd.extend_from_slice(&b.0);
}

/// Inflate from `at`, returning (compressed bytes consumed, produced bytes, crc, ok, error).
fn probe_deflate(f: &File, at: u64, limit: u64, sink: Option<&mut Vec<u8>>) -> (u64, u64, u32, bool) {
    let mut d = Decompress::new(false);
    let (mut inb, mut outb) = (vec![0u8; 256 << 10], vec![0u8; 256 << 10]);
    let mut crc = crc32fast::Hasher::new();
    let (mut pos, mut produced) = (at, 0u64);
    let mut sink = sink;
    loop {
        let n = ((limit.saturating_sub(pos)) as usize).min(inb.len());
        if n == 0 { return (d.total_in(), produced, crc.finalize(), false); }
        if f.read_exact_at(&mut inb[..n], pos).is_err() { return (d.total_in(), produced, crc.finalize(), false); }
        let mut consumed = 0;
        loop {
            let (bi, bo) = (d.total_in(), d.total_out());
            let st = d.decompress(&inb[consumed..n], &mut outb, FlushDecompress::None);
            let (ci, co) = ((d.total_in() - bi) as usize, (d.total_out() - bo) as usize);
            consumed += ci;
            crc.update(&outb[..co]);
            produced += co as u64;
            if let Some(s) = sink.as_deref_mut() { if s.len() < (256 << 20) { s.extend_from_slice(&outb[..co]); } }
            match st {
                Ok(Status::StreamEnd) => return (d.total_in(), produced, crc.finalize(), true),
                Ok(_) => { if consumed >= n && co == 0 { break; } if ci == 0 && co == 0 { break; } }
                Err(_) => return (d.total_in(), produced, crc.finalize(), false),
            }
        }
        pos += n as u64;
    }
}

pub fn repair_zip(input: &Path, output: &Path, cancel: &CancelToken) -> Result<RepairReport> {
    let f = File::open(input)?;
    let len = f.metadata()?.len();
    let mut rep = RepairReport { strategy: "central-directory rebuild", ..Default::default() };
    let mut w = BufWriter::with_capacity(1 << 20, File::create(output)?);
    let mut cd = Vec::new();
    let mut pos_out = 0u64;
    let mut at = 0u64;
    let mut count = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    let sigs = [SIG_LOCAL.to_le_bytes(), SIG_CENTRAL.to_le_bytes()];
    let copy = |w: &mut BufWriter<File>, mut from: u64, mut n: u64, buf: &mut Vec<u8>| -> Result<()> {
        while n > 0 { let k = (n as usize).min(buf.len()); f.read_exact_at(&mut buf[..k], from)?; w.write_all(&buf[..k])?; from += k as u64; n -= k as u64; }
        Ok(())
    };
    let r = (|| -> Result<()> {
        loop {
            cancel.check()?;
            let Some((p, sig)) = find_sig(&f, at, len, &sigs)? else { break };
            if sig == SIG_CENTRAL.to_le_bytes() { break; }
            let mut h = [0u8; 30];
            if f.read_exact_at(&mut h, p).is_err() { rep.lost.push(("<unknown entry>".into(), "local header truncated".into())); break; }
            let (flags, method, time, date) = (rd16(&h, 6), rd16(&h, 8), rd16(&h, 10), rd16(&h, 12));
            let (crc, csize, usize_) = (rd32(&h, 14), rd32(&h, 18) as u64, rd32(&h, 22) as u64);
            let (nl, el) = (rd16(&h, 26) as usize, rd16(&h, 28) as usize);
            let mut ne = vec![0u8; nl + el];
            if f.read_exact_at(&mut ne, p + 30).is_err() { rep.lost.push(("<unknown entry>".into(), "local header truncated".into())); break; }
            let name = ne[..nl].to_vec();
            let extra = ne[nl..].to_vec();
            let disp = String::from_utf8_lossy(&name).into_owned();
            let data_at = p + 30 + (nl + el) as u64;
            let mut rec = Rec { hdr_flags: flags, method, time, date, crc, csize, usize: usize_, name: name.clone(), extra: extra.clone(), off: pos_out };
            let has_desc = flags & FLAG_DESCRIPTOR != 0;
            let ok_end;
            let mut damaged: Option<String> = None;
            let mut salvage: Option<Vec<u8>> = None;
            if csize == U32_MAX || usize_ == U32_MAX { rep.lost.push((disp, "ZIP64 entries are not supported by repair".into())); at = p + 4; continue; }
            if !has_desc {
                let end = data_at + csize;
                if end > len { damaged = Some("data truncated".into()); ok_end = len; }
                else {
                    ok_end = end;
                    if method == M_DEFLATE && flags & FLAG_ENCRYPTED == 0 {
                        let mut part = Vec::new();
                        let (_, prod, c, ok) = probe_deflate(&f, data_at, end, Some(&mut part));
                        if !ok || prod != usize_ || c != crc { damaged = Some("data fails integrity check".into()); salvage = Some(part); }
                    } else if method == M_STORE && flags & FLAG_ENCRYPTED == 0 {
                        let mut h2 = crc32fast::Hasher::new(); let mut o = data_at; let mut left = csize;
                        while left > 0 { let k = (left as usize).min(buf.len()); f.read_exact_at(&mut buf[..k], o)?; h2.update(&buf[..k]); o += k as u64; left -= k as u64; }
                        if h2.finalize() != crc { damaged = Some("stored data fails CRC".into()); }
                    }
                }
            } else {
                // sizes live in the data descriptor: find the end of the stream
                if method == M_DEFLATE && flags & FLAG_ENCRYPTED == 0 {
                    let mut part = Vec::new();
                    let (used, prod, c, ok) = probe_deflate(&f, data_at, len, Some(&mut part));
                    if ok {
                        let mut d = [0u8; 24];
                        let _ = f.read_at(&mut d, data_at + used)?;
                        let skip = if rd32(&d, 0) == SIG_DESC { 4 } else { 0 };
                        rec.crc = c; rec.csize = used; rec.usize = prod;
                        // local sizes may be 32/64-bit in the descriptor; accept 32-bit form, fall back to 64-bit
                        let dlen = if rd32(&d, skip + 4) as u64 == used && rd32(&d, skip + 8) as u64 == prod { 12 } else { 20 };
                        ok_end = data_at + used + (skip + dlen) as u64;
                    } else { damaged = Some("deflate stream damaged".into()); salvage = Some(part); ok_end = len; }
                } else {
                    // unknown end: run to next signature
                    let next = find_sig(&f, data_at, len, &sigs)?.map(|x| x.0).unwrap_or(len);
                    rec.csize = next - data_at; ok_end = next; rec.hdr_flags &= !FLAG_DESCRIPTOR;
                }
            }
            if let Some(why) = damaged {
                // salvage whatever inflated cleanly as a stored `.recovered` entry
                match salvage.filter(|s| !s.is_empty()) {
                    Some(bytes) => {
                        let mut nm = name.clone(); nm.extend_from_slice(b".recovered");
                        let c = crc32fast::hash(&bytes);
                        let r2 = Rec { hdr_flags: flags & FLAG_EFS, method: M_STORE, time, date, crc: c, csize: bytes.len() as u64, usize: bytes.len() as u64, name: nm.clone(), extra: vec![], off: pos_out };
                        let mut lh = Le::default();
                        lh.u32(SIG_LOCAL).u16(20).u16(r2.hdr_flags).u16(M_STORE).u16(time).u16(date).u32(c).u32(bytes.len() as u32).u32(bytes.len() as u32).u16(nm.len() as u16).u16(0).bytes(&nm);
                        w.write_all(&lh.0)?; w.write_all(&bytes)?;
                        pos_out += (lh.0.len() + bytes.len()) as u64;
                        push_central(&mut cd, &r2); count += 1;
                        rep.partial.push((disp, format!("{why}; saved {} readable bytes", bytes.len())));
                    }
                    None => rep.lost.push((disp, why)),
                }
                at = ok_end.max(p + 4);
                continue;
            }
            // intact: copy verbatim (header + data [+ descriptor])
            let total = ok_end - p;
            copy(&mut w, p, total, &mut buf)?;
            pos_out += total;
            push_central(&mut cd, &rec);
            count += 1;
            rep.recovered.push(disp);
            at = ok_end.max(p + 4);
        }
        if count >= 0xFFFF || pos_out >= U32_MAX { return Err(Error::new(ErrorKind::FourGbLimit, "repair of ZIP64-sized archives is not supported")); }
        w.write_all(&cd)?;
        let mut e = Le::default();
        e.u32(SIG_EOCD).u16(0).u16(0).u16(count as u16).u16(count as u16).u32(cd.len() as u32).u32(pos_out as u32).u16(0);
        w.write_all(&e.0)?;
        w.flush()?;
        w.get_ref().sync_all()?;
        Ok(())
    })();
    drop(w);
    if let Err(e) = r { let _ = std::fs::remove_file(output); return Err(e); }
    Ok(rep)
}
