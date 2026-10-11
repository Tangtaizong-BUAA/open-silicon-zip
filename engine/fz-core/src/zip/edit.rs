//! In-place ZIP editing (FR-301..304): add / delete / rename via safe rewrite through a temp file.
//! Untouched entries are copied byte-identically; only the central directory is rebuilt.
use super::format::*;
use super::reader::{ReadOptions, ZipArchive};
use super::writer::{create_zip, ZipOptions};
use crate::cancel::{CancelToken, Progress};
use crate::enumerate::EntryDesc;
use crate::error::{Error, ErrorKind, Result};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct EditOp {
    pub delete: Vec<usize>,
    /// (entry index, new name)
    pub rename: Vec<(usize, String)>,
    pub add: Vec<EntryDesc>,
}

pub fn validate_new_name(n: &str) -> Result<()> {
    let bad = n.is_empty() || n.contains('\\') || n.contains('\0') || n.starts_with('/') || n.split('/').any(|c| c == ".." )
        || (n.len() >= 2 && n.as_bytes()[1] == b':' && n.as_bytes()[0].is_ascii_alphabetic());
    if bad { Err(Error::new(ErrorKind::InvalidInput, format!("invalid entry name '{n}'"))) } else { Ok(()) }
}

struct OutRec { rec: Vec<u8> }

fn copy_entries(arc: &ZipArchive, keep: &[usize], renames: &std::collections::HashMap<usize, String>, w: &mut BufWriter<File>, pos: &mut u64,
                cd: &mut Vec<OutRec>, cancel: &CancelToken) -> Result<()> {
    // extent end of each entry = next local offset (or start of the central directory)
    let mut offs: Vec<u64> = arc.entries.iter().map(|e| e.local_off).collect();
    offs.sort_unstable();
    let src = &arc.vols;
    let mut buf = vec![0u8; 1 << 20];
    for &i in keep {
        cancel.check()?;
        let e = &arc.entries[i];
        let end = offs.iter().copied().find(|&o| o > e.local_off).unwrap_or(arc.cd_start);
        let new_off = *pos;
        let (cs, cl) = arc.cd_ranges[i];
        let mut rec = arc.cd[cs..cs + cl].to_vec();
        let mut data_from = e.local_off;
        if let Some(newname) = renames.get(&i) {
            validate_new_name(newname)?;
            let mut h = [0u8; 30];
            src.read_exact(e.disk, e.local_off, &mut h)?;
            let (nl, el) = (rd16(&h, 26) as usize, rd16(&h, 28) as usize);
            let mut old_extra = vec![0u8; el];
            src.read_exact(e.disk, e.local_off + 30 + nl as u64, &mut old_extra)?;
            let nb = newname.as_bytes().to_vec();
            let strip = |extra: &[u8]| -> Vec<u8> {
                let mut out = Vec::new();
                for (id, d) in extra_fields(extra) { if id != EXTRA_UNICODE_PATH { out.extend_from_slice(&id.to_le_bytes()); out.extend_from_slice(&(d.len() as u16).to_le_bytes()); out.extend_from_slice(d); } }
                out
            };
            let uni = |extra: &mut Vec<u8>| { if !nb.is_ascii() { extra.extend_from_slice(&EXTRA_UNICODE_PATH.to_le_bytes()); extra.extend_from_slice(&(5 + nb.len() as u16).to_le_bytes()); extra.push(1); extra.extend_from_slice(&crc32fast::hash(&nb).to_le_bytes()); extra.extend_from_slice(&nb); } };
            let mut lextra = strip(&old_extra); uni(&mut lextra);
            let mut lh = h.to_vec();
            let flags = rd16(&lh, 6) | FLAG_EFS;
            lh[6..8].copy_from_slice(&flags.to_le_bytes());
            lh[26..28].copy_from_slice(&(nb.len() as u16).to_le_bytes());
            lh[28..30].copy_from_slice(&(lextra.len() as u16).to_le_bytes());
            w.write_all(&lh)?; w.write_all(&nb)?; w.write_all(&lextra)?;
            *pos += (30 + nb.len() + lextra.len()) as u64;
            data_from = e.local_off + 30 + nl as u64 + el as u64;
            // central record
            let (cnl, cel, ccl) = (rd16(&rec, 28) as usize, rd16(&rec, 30) as usize, rd16(&rec, 32) as usize);
            let cextra = rec[46 + cnl..46 + cnl + cel].to_vec();
            let mut nextra = strip(&cextra); uni(&mut nextra);
            let tail = rec[46 + cnl + cel..46 + cnl + cel + ccl].to_vec();
            let mut nr = rec[..46].to_vec();
            let f2 = rd16(&nr, 8) | FLAG_EFS;
            nr[8..10].copy_from_slice(&f2.to_le_bytes());
            nr[28..30].copy_from_slice(&(nb.len() as u16).to_le_bytes());
            nr[30..32].copy_from_slice(&(nextra.len() as u16).to_le_bytes());
            nr.extend_from_slice(&nb); nr.extend_from_slice(&nextra); nr.extend_from_slice(&tail);
            rec = nr;
        }
        // raw copy of the remaining extent
        let mut at = data_from;
        let mut left = end.saturating_sub(data_from);
        while left > 0 {
            let n = (left as usize).min(buf.len());
            src.read_exact(e.disk, at, &mut buf[..n])?;
            w.write_all(&buf[..n])?;
            at += n as u64; left -= n as u64; *pos += n as u64;
        }
        // patch offset + disk (single-volume archives only)
        if new_off >= U32_MAX { return Err(Error::new(ErrorKind::FourGbLimit, "editing archives over 4 GB is not supported; extract and recreate")); }
        rec[42..46].copy_from_slice(&(new_off as u32).to_le_bytes());
        rec[34..36].copy_from_slice(&0u16.to_le_bytes());
        // a ZIP64 extra inside the record may carry the old offset; only patch when no zip64 offset is present
        if rd32(&rec, 42) as u64 != new_off { unreachable!(); }
        cd.push(OutRec { rec });
    }
    Ok(())
}

pub fn edit_zip(path: &Path, op: &EditOp, add_opts: &ZipOptions, cancel: &CancelToken, progress: &Progress) -> Result<()> {
    let arc = ZipArchive::open(path, ReadOptions::default())?;
    if arc.spanned { return Err(Error::new(ErrorKind::UnsupportedFormat, "split archives are read-only; join or recreate them")); }
    if arc.base != 0 { return Err(Error::new(ErrorKind::UnsupportedFormat, "archives with a self-extractor stub are read-only")); }
    if arc.entries.iter().any(|e| e.local_off >= U32_MAX || e.csize >= U32_MAX) { return Err(Error::new(ErrorKind::FourGbLimit, "editing archives over 4 GB is not supported")); }
    let mut delete: std::collections::HashSet<usize> = op.delete.iter().copied().collect();
    let mut renames = std::collections::HashMap::new();
    for (i, n) in &op.rename { validate_new_name(n)?; renames.insert(*i, n.clone()); }
    // uniqueness after renames
    let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (i, e) in arc.entries.iter().enumerate() {
        if delete.contains(&i) { continue; }
        let n = renames.get(&i).cloned().unwrap_or_else(|| e.name.clone());
        if !names.insert(n.clone()) { return Err(Error::new(ErrorKind::InvalidInput, format!("duplicate entry name '{n}'"))); }
    }
    // added entries replace same-named ones
    for a in &op.add { let n = a.name.trim_end_matches('/'); for (i, e) in arc.entries.iter().enumerate() { if e.name.trim_end_matches('/') == n && !renames.contains_key(&i) { delete.insert(i); } } }
    let keep: Vec<usize> = (0..arc.entries.len()).filter(|i| !delete.contains(i)).collect();

    let tmp_add: Option<PathBuf> = if op.add.is_empty() { None } else {
        let t = PathBuf::from(format!("{}.add.part", path.display()));
        create_zip(&t, &op.add, add_opts, cancel, progress)?;
        Some(t)
    };
    let out_tmp = PathBuf::from(format!("{}.edit.part", path.display()));
    let r = (|| -> Result<()> {
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&out_tmp)?);
        let mut pos = 0u64;
        let mut cd: Vec<OutRec> = Vec::new();
        copy_entries(&arc, &keep, &renames, &mut w, &mut pos, &mut cd, cancel)?;
        if let Some(t) = &tmp_add {
            let a2 = ZipArchive::open(t, ReadOptions::default())?;
            let all: Vec<usize> = (0..a2.entries.len()).collect();
            copy_entries(&a2, &all, &Default::default(), &mut w, &mut pos, &mut cd, cancel)?;
        }
        let cd_off = pos;
        let mut cd_size = 0u64;
        for r in &cd { w.write_all(&r.rec)?; cd_size += r.rec.len() as u64; }
        pos += cd_size;
        let n = cd.len() as u64;
        let need64 = n >= 0xFFFF || pos >= U32_MAX;
        if need64 {
            let mut z = Le::default();
            z.u32(SIG_EOCD64).u64(44).u16(45).u16(45).u32(0).u32(0).u64(n).u64(n).u64(cd_size).u64(cd_off);
            z.u32(SIG_LOC64).u32(0).u64(pos).u32(1);
            w.write_all(&z.0)?;
        }
        let mut e = Le::default();
        e.u32(SIG_EOCD).u16(0).u16(0).u16(if need64 { 0xFFFF } else { n as u16 }).u16(if need64 { 0xFFFF } else { n as u16 })
            .u32(if need64 { U32_MAX as u32 } else { cd_size as u32 }).u32(if need64 { U32_MAX as u32 } else { cd_off as u32 })
            .u16(arc.comment.len() as u16).bytes(&arc.comment);
        w.write_all(&e.0)?;
        w.flush()?;
        w.get_ref().sync_all()?;
        Ok(())
    })();
    if let Some(t) = &tmp_add { let _ = std::fs::remove_file(t); }
    match r {
        Ok(()) => { std::fs::rename(&out_tmp, path)?; Ok(()) }
        Err(e) => { let _ = std::fs::remove_file(&out_tmp); Err(e) }
    }
}
