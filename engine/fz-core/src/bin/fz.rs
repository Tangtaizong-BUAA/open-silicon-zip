//! `fz` command-line front-end; also the process the Swift app drives (line-delimited JSON with --json).
use clap::{Parser, Subcommand};
use fz_core::cancel::{CancelToken, Progress};
use fz_core::enumerate::{enumerate, EnumOptions, SymlinkPolicy};
use fz_core::error::{Error, ErrorKind};
use fz_core::formats::{self, codecs::Codec, sevenz, tar};
use fz_core::zip::{self, Encryption, Level, Method, ZipOptions};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "fz", about = "Open Silicon Zip engine CLI")]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    /// Read the password as UTF-8 from stdin until EOF (maximum 64 KiB).
    #[arg(long, global = true)]
    password_stdin: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Create {
        out: PathBuf,
        inputs: Vec<PathBuf>,
        #[arg(long)] format: Option<String>,
        #[arg(long, default_value = "normal")] level: String,
        #[arg(long)] method: Option<String>,
        #[arg(long)] password: Option<String>,
        #[arg(long, default_value = "aes256")] crypto: String,
        #[arg(long)] split: Option<String>,
        #[arg(long, default_value_t = 0)] threads: usize,
        #[arg(long)] exclude: Vec<String>,
        #[arg(long)] include_junk: bool,
        #[arg(long, default_value = "on")] high_speed: String,
        #[arg(long)] dereference: bool,
        #[arg(long)] no_wrap: bool,
        #[arg(long)] codepage: Option<String>,
        #[arg(long)] comment: Option<String>,
        #[arg(long)] solid: Option<String>,
        #[arg(long)] encrypt_names: bool,
    },
    Extract {
        archive: PathBuf,
        #[arg(short, long)] output: PathBuf,
        #[arg(long)] smart: bool,
        #[arg(long)] password: Option<String>,
        #[arg(long)] codepage: Option<String>,
        #[arg(long, default_value = "rename")] overwrite: String,
        entries: Vec<String>,
    },
    List { archive: PathBuf, #[arg(long)] codepage: Option<String>, #[arg(long)] password: Option<String> },
    Test { archive: PathBuf, #[arg(long)] password: Option<String> },
    /// Add / delete / rename entries in a ZIP (safe rewrite).
    Edit {
        archive: PathBuf,
        #[arg(long)] add: Vec<PathBuf>,
        #[arg(long)] delete: Vec<String>,
        /// OLD=NEW
        #[arg(long)] rename: Vec<String>,
    },
    /// Salvage a damaged ZIP into a new file.
    Repair { archive: PathBuf, #[arg(short, long)] output: PathBuf },
    /// Brute-force password recovery for ZIP.
    Recover {
        archive: PathBuf,
        #[arg(long, default_value = "digits,lower")] charset: String,
        #[arg(long, default_value = "")] prefix: String,
        #[arg(long, default_value_t = 1)] min: usize,
        #[arg(long, default_value_t = 6)] max: usize,
        #[arg(long, default_value_t = 0)] threads: usize,
    },
    /// Split any file into NAME.001, NAME.002, ...
    Split { file: PathBuf, #[arg(long)] size: String },
    /// Join NAME.001... back into NAME.
    Join { first: PathBuf, #[arg(short, long)] output: Option<PathBuf> },
    Hash { files: Vec<PathBuf>, #[arg(long)] expect: Option<String> },
}

struct Out { json: bool }
impl Out {
    fn emit(&self, v: serde_json::Value) { if self.json { println!("{v}"); } }
    fn say(&self, s: impl AsRef<str>) { if !self.json { println!("{}", s.as_ref()); } }
}

fn parse_size(s: &str) -> Result<u64, Error> {
    let t = s.trim().to_ascii_lowercase();
    let (n, mul) = if let Some(x) = t.strip_suffix("gb") { (x, 1u64 << 30) } else if let Some(x) = t.strip_suffix("mb") { (x, 1 << 20) }
        else if let Some(x) = t.strip_suffix("kb") { (x, 1 << 10) } else if let Some(x) = t.strip_suffix('g') { (x, 1 << 30) }
        else if let Some(x) = t.strip_suffix('m') { (x, 1 << 20) } else if let Some(x) = t.strip_suffix('k') { (x, 1 << 10) } else { (t.as_str(), 1) };
    n.trim().parse::<f64>().map(|v| (v * mul as f64) as u64).map_err(|_| Error::new(ErrorKind::InvalidSplitSize, format!("bad size '{s}'")))
}

fn level(s: &str) -> Level { match s { "store" => Level::Store, "fast" => Level::Fast, "max" | "maximum" => Level::Maximum, _ => Level::Normal } }

/// Run `f` while a thread streams progress events (>= 5 Hz).
fn with_progress<T>(out: &Out, p: &Progress, f: impl FnOnce() -> T) -> T {
    // Text-mode commands have no periodic progress output to publish.
    if !out.json { return f(); }
    let stop = Arc::new(AtomicBool::new(false));
    let json = out.json;
    let (p2, s2) = (p.clone(), stop.clone());
    let h = std::thread::spawn(move || {
        while !s2.load(Ordering::Acquire) {
            if json {
                let s = p2.snapshot();
                println!("{}", json!({"event":"progress","phase":s.phase,"current":s.current,"bytes_read":s.bytes_read,"bytes_written":s.bytes_written,
                    "bytes_total":s.bytes_total,"files_done":s.files_done,"files_total":s.files_total}));
            }
            std::thread::park_timeout(std::time::Duration::from_millis(150));
        }
    });
    let r = f();
    stop.store(true, Ordering::Release);
    // unpark retains a wake-up token if the worker has not parked yet.
    h.thread().unpark();
    let _ = h.join();
    r
}

fn run(mut cli: Cli) -> Result<(), Error> {
    if cli.password_stdin {
        let slot = match &mut cli.cmd {
            Cmd::Create { password, .. } | Cmd::Extract { password, .. } |
            Cmd::List { password, .. } | Cmd::Test { password, .. } => password,
            _ => return Err(Error::new(ErrorKind::InvalidInput, "this command does not accept a password")),
        };
        if slot.is_some() { return Err(Error::new(ErrorKind::InvalidInput, "use only one password source")); }
        use std::io::Read;
        let mut bytes = Vec::new();
        std::io::stdin().take(65537).read_to_end(&mut bytes)?;
        if bytes.len() > 65536 { return Err(Error::new(ErrorKind::InvalidInput, "password exceeds 64 KiB")); }
        *slot = Some(String::from_utf8(bytes).map_err(|_| Error::new(ErrorKind::InvalidInput, "password must be UTF-8"))?);
    }
    let out = Out { json: cli.json };
    let cancel = CancelToken::new();
    #[cfg(unix)]
    if !matches!(&cli.cmd, Cmd::Hash { .. } | Cmd::Split { .. } | Cmd::Join { .. }) {
        cancel.install_signal_handlers()?;
    }
    let progress = Progress::new();
    match cli.cmd {
        Cmd::Create { out: dest, inputs, format, level: lv, method, password, crypto, split, threads, exclude, include_junk, high_speed, dereference, no_wrap, codepage, comment, solid, encrypt_names } => {
            let fmt = format.unwrap_or_else(|| dest.extension().and_then(|e| e.to_str()).unwrap_or("zip").to_ascii_lowercase());
            let mut eo = EnumOptions::default();
            eo.symlinks = if dereference { SymlinkPolicy::Dereference } else { SymlinkPolicy::Store };
            eo.include_apple_junk = include_junk;
            eo.wrap_single_root = !no_wrap;
            eo.deterministic = true;
            if !exclude.is_empty() { eo.exclude = Some(fz_core::pathsafe::ExcludeFilter::new(&exclude)); }
            progress.set_phase("scanning");
            let entries = enumerate(&inputs, &eo, &cancel)?;
            match fmt.as_str() {
                "zip" | "zipx" => {
                    let mut o = ZipOptions::default();
                    o.level = level(&lv);
                    o.method = match method.as_deref() { Some("store") => Method::Store, Some("zstd") => Method::Zstd, Some("bzip2") => Method::Bzip2, Some("lzma") => Method::Lzma, Some("ppmd") => Method::Ppmd, _ if fmt == "zipx" => Method::Zstd, _ => Method::Deflate };
                    o.threads = threads;
                    o.high_speed = match high_speed.as_str() { "off" => zip::HighSpeed::Off, "aggressive" => zip::HighSpeed::Aggressive, _ => zip::HighSpeed::On };
                    if let Some(p) = password {
                        o.encryption = match crypto.as_str() {
                            "zipcrypto" => Encryption::ZipCrypto(p),
                            "aes128" => Encryption::Aes { strength: 1, password: p },
                            "aes192" => Encryption::Aes { strength: 2, password: p },
                            _ => Encryption::Aes { strength: 3, password: p },
                        };
                    }
                    if let Some(s) = split { o.split_size = Some(parse_size(&s)?); }
                    o.legacy_codepage = codepage;
                    if let Some(c) = comment { o.comment = c; }
                    let st = with_progress(&out, &progress, || zip::create_zip(&dest, &entries, &o, &cancel, &progress))?;
                    out.say(format!("created {} ({} entries, {} -> {} bytes, {} volume(s), {} stored by high-speed)", dest.display(), st.entries, st.bytes_in, st.bytes_out, st.volumes, st.stored_by_skiplist));
                }
                "7z" => {
                    let mut o = sevenz::SevenOptions::default();
                    o.level = level(&lv);
                    o.threads = threads;
                    o.high_speed = high_speed != "off";
                    o.method = match method.as_deref() { Some("lzma") => sevenz::SzMethod::Lzma, Some("ppmd") => sevenz::SzMethod::Ppmd, Some("bzip2") => sevenz::SzMethod::Bzip2,
                        Some("zstd") => sevenz::SzMethod::Zstd, Some("deflate") => sevenz::SzMethod::Deflate, Some("copy") | Some("store") => sevenz::SzMethod::Copy, _ => sevenz::SzMethod::Lzma2 };
                    o.solid = match solid.as_deref() { Some("off") | Some("none") => sevenz::Solid::None, Some("on") | Some("single") => sevenz::Solid::Single,
                        Some(x) => sevenz::Solid::Bytes(parse_size(x)?), None => sevenz::Solid::Auto };
                    o.password = password;
                    o.encrypt_names = encrypt_names;
                    if let Some(s) = split { o.split_size = Some(parse_size(&s)?); }
                    let len = with_progress(&out, &progress, || sevenz::create_7z(&dest, &entries, &o, &cancel, &progress))?;
                    out.say(format!("created {} ({} entries, {} bytes)", dest.display(), entries.len(), len));
                }
                f if matches!(f, "tar" | "tgz" | "tbz" | "tbz2" | "txz" | "tlz" | "tzst" | "tlz4" | "tbr") || f.starts_with("tar.") => {
                    let ext = f.strip_prefix("tar.").unwrap_or(f);
                    let codec = if f == "tar" { Codec::None } else { Codec::from_ext(ext).ok_or_else(|| Error::new(ErrorKind::UnsupportedFormat, f.to_string()))? };
                    let mut o = tar::TarOptions::default();
                    o.codec = codec; o.level = level(&lv); o.threads = threads; o.store_symlinks = !dereference;
                    let len = with_progress(&out, &progress, || tar::create_tar(&dest, &entries, &o, &cancel, &progress))?;
                    out.say(format!("created {} ({} entries, {} bytes)", dest.display(), entries.len(), len));
                }
                f if Codec::from_ext(f).is_some() => {
                    let files: Vec<_> = entries.iter().filter(|e| e.kind == fz_core::enumerate::EntryKind::File).collect();
                    if files.len() != 1 { return Err(Error::new(ErrorKind::InvalidInput, "single-stream formats hold one file; use a tar variant (tgz, txz, tzst) for many")); }
                    let len = with_progress(&out, &progress, || tar::create_single_stream(&dest, &files[0].abs, Codec::from_ext(f).unwrap(), level(&lv), threads, &cancel, &progress))?;
                    out.say(format!("created {} ({} bytes)", dest.display(), len));
                }
                other => return Err(Error::new(ErrorKind::UnsupportedFormat, format!("cannot create '{other}'"))),
            }
            out.emit(json!({"event":"done","ok":true}));
        }
        Cmd::List { archive, codepage, password } => {
            let items = formats::list(&archive, password.as_deref(), codepage.as_deref(), &cancel)?;
            for e in &items {
                if cli.json {
                    out.emit(json!({"event":"entry","name":e.name,"size":e.size,"csize":e.csize,"mtime":e.mtime,"method":e.method,"encrypted":e.encrypted,
                        "crc":e.crc.map(|c| format!("{:08x}", c)),"attrs":format!("{:o}", e.attrs),"is_dir":e.is_dir}));
                } else { println!("{:>12} {:>12}  {}", e.size, e.csize, e.name); }
            }
            out.emit(json!({"event":"done","ok":true}));
        }
        Cmd::Extract { archive, output, smart, password, codepage, overwrite, entries } => {
            let mut eo = zip::ExtractOptions::new(output);
            eo.smart = smart;
            eo.password = password;
            eo.overwrite = match overwrite.as_str() { "overwrite" => zip::Overwrite::Overwrite, "skip" => zip::Overwrite::Skip, _ => zip::Overwrite::Rename };
            if !entries.is_empty() {
                let items = formats::list(&archive, eo.password.as_deref(), codepage.as_deref(), &cancel)?;
                eo.selected = Some(items.iter().enumerate().filter(|(_, e)| entries.iter().any(|n| e.name == *n || e.name.starts_with(&format!("{}/", n.trim_end_matches('/'))))).map(|(i, _)| i).collect());
            }
            let r = with_progress(&out, &progress, || formats::extract(&archive, &eo, codepage.as_deref(), &cancel, &progress))?;
            out.say(format!("extracted {} files ({} bytes) to {}; skipped {}, refused {}, failed {}", r.extracted, r.bytes, r.root.display(), r.skipped, r.unsafe_refused.len(), r.failed.len()));
            for (n, e) in &r.failed { out.say(format!("  FAILED {n}: {e}")); }
            for n in &r.unsafe_refused { out.say(format!("  REFUSED (unsafe path) {n}")); }
            out.emit(json!({"event":"done","ok":r.failed.is_empty() && r.unsafe_refused.is_empty() && r.skipped == 0,"extracted":r.extracted,"skipped":r.skipped,"refused":r.unsafe_refused,"failed":r.failed,"root":r.root}));
            if !r.failed.is_empty() || !r.unsafe_refused.is_empty() || r.skipped > 0 { std::process::exit(2); }
        }
        Cmd::Test { archive, password } => {
            let res = with_progress(&out, &progress, || formats::test(&archive, password.as_deref(), &cancel, &progress))?;
            let bad: Vec<_> = res.iter().filter(|(_, r)| r.is_err()).collect();
            for (n, r) in &bad { out.say(format!("ERROR {n}: {}", r.as_ref().unwrap_err())); }
            if bad.is_empty() { out.say("No errors found"); }
            out.emit(json!({"event":"done","ok":bad.is_empty(),"errors":bad.len(),"tested":res.len()}));
            if !bad.is_empty() { std::process::exit(2); }
        }
        Cmd::Edit { archive, add, delete, rename } => {
            let a = zip::ZipArchive::open(&archive, zip::ReadOptions::default())?;
            let mut op = zip::edit::EditOp::default();
            for d in &delete {
                let hits: Vec<usize> = a.entries.iter().filter(|e| e.name == *d || e.name.starts_with(&format!("{}/", d.trim_end_matches('/')))).map(|e| e.index).collect();
                if hits.is_empty() { return Err(Error::new(ErrorKind::InvalidInput, format!("no such entry: {d}"))); }
                op.delete.extend(hits);
            }
            for r in &rename {
                let (o, n) = r.split_once('=').ok_or_else(|| Error::new(ErrorKind::InvalidInput, "use --rename OLD=NEW"))?;
                let e = a.entries.iter().find(|e| e.name == o).ok_or_else(|| Error::new(ErrorKind::InvalidInput, format!("no such entry: {o}")))?;
                op.rename.push((e.index, n.to_string()));
            }
            if !add.is_empty() { let mut eo = EnumOptions::default(); eo.deterministic = true; op.add = enumerate(&add, &eo, &cancel)?; }
            drop(a);
            zip::edit::edit_zip(&archive, &op, &ZipOptions::default(), &cancel, &progress)?;
            out.say("archive updated");
            out.emit(json!({"event":"done","ok":true}));
        }
        Cmd::Repair { archive, output } => {
            let r = zip::repair::repair_zip(&archive, &output, &cancel)?;
            out.say(format!("{}: recovered {}, partial {}, lost {}", r.strategy, r.recovered.len(), r.partial.len(), r.lost.len()));
            for (n, w) in &r.partial { out.say(format!("  PARTIAL {n}: {w}")); }
            for (n, w) in &r.lost { out.say(format!("  LOST {n}: {w}")); }
            out.emit(json!({"event":"done","ok":r.lost.is_empty(),"recovered":r.recovered,"partial":r.partial,"lost":r.lost}));
        }
        Cmd::Recover { archive, charset, prefix, min, max, threads } => {
            use fz_core::recover::*;
            let mut cs = Charset { chars: vec![] };
            for part in charset.split(',') {
                let c = match part { "digits" => Charset::digits(), "lower" => Charset::lower(), "upper" => Charset::upper(), "special" => Charset::special(), "space" => Charset::space(), "all" => Charset::all(), x => Charset::custom(x) };
                cs = cs.merge(&c);
            }
            let cfg = RecoverConfig { charset: cs, prefix, min_len: min, max_len: max, threads, resume: None };
            let stats = RecoverStats::default();
            let st2 = stats.clone();
            let json = out.json;
            let stop = Arc::new(AtomicBool::new(false)); let s2 = stop.clone();
            let h = std::thread::spawn(move || while !s2.load(Ordering::Relaxed) { let s = st2.snapshot(); if json { println!("{}", json!({"event":"progress","phase":"recovering","tested":s.tested,"per_second":s.per_second,"elapsed":s.elapsed_secs,"eta":s.eta_secs})); } std::thread::sleep(std::time::Duration::from_millis(500)); });
            let r = recover(&archive, &cfg, &cancel, &PauseToken::default(), &stats);
            stop.store(true, Ordering::Relaxed); let _ = h.join();
            match r? {
                Outcome::Found(p) => { out.say(format!("PASSWORD FOUND: {p}")); out.emit(json!({"event":"done","ok":true,"password":p})); }
                Outcome::Exhausted => { out.say("password not found in the search space"); out.emit(json!({"event":"done","ok":false,"exhausted":true})); std::process::exit(3); }
                Outcome::Cancelled(cp) => { out.emit(json!({"event":"done","ok":false,"checkpoint":cp.to_json()})); }
            }
        }
        Cmd::Split { file, size } => {
            let sz = parse_size(&size)?;
            if sz < fz_core::zip::writer::MIN_SPLIT { return Err(Error::new(ErrorKind::InvalidSplitSize, "minimum volume size is 100 KB")); }
            let parts = sevenz::split_file(&file, &file, sz)?;
            out.say(format!("wrote {} volumes", parts.len()));
            out.emit(json!({"event":"done","ok":true,"volumes":parts}));
        }
        Cmd::Join { first, output } => {
            let dir = output.as_ref().and_then(|o| o.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| first.parent().map(|p| p.to_path_buf()).unwrap_or_default());
            let dir = if dir.as_os_str().is_empty() { PathBuf::from(".") } else { dir };
            let joined = sevenz::join_volumes(&first, &dir)?;
            let dest = output.unwrap_or(joined.clone());
            if dest != joined { std::fs::rename(&joined, &dest)?; }
            out.say(format!("joined into {}", dest.display()));
            out.emit(json!({"event":"done","ok":true,"output":dest}));
        }
        Cmd::Hash { files, expect } => {
            use rayon::prelude::*;
            let rs: Vec<_> = files.par_iter().map(|f| (f.clone(), fz_core::hash::digest_file(f))).collect();
            for (f, d) in rs {
                let d = d?;
                if let Some(x) = &expect { out.say(format!("{}: {:?}", f.display(), fz_core::hash::compare_expected(&d, x))); }
                out.emit(json!({"event":"hash","file":f,"size":d.size,"crc32":d.crc32,"crc64":d.crc64,"md5":d.md5,"sha1":d.sha1,"sha256":d.sha256}));
                out.say(format!("{}\n  CRC32 {}\n  CRC64 {}\n  MD5   {}\n  SHA1  {}\n  SHA256 {}", f.display(), d.crc32, d.crc64, d.md5, d.sha1, d.sha256));
            }
            out.emit(json!({"event":"done","ok":true}));
        }
    }
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    if let Err(e) = run(cli) {
        if json { println!("{}", json!({"event":"error","code":e.kind.code(),"message":e.detail,"guidance":e.kind.guidance()})); }
        else { eprintln!("error: {e}\n  {}", e.kind.guidance()); }
        std::process::exit(if e.kind == ErrorKind::Cancelled { 130 } else { 1 });
    }
}
