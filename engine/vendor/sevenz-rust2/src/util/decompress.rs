#[cfg(target_os = "macos")]
use std::os::macos::fs::FileTimesExt;
#[cfg(windows)]
use std::os::windows::fs::FileTimesExt;
use std::{
    fs::FileTimes,
    io::{Read, Seek},
    path::{Path, PathBuf},
};

use crate::{Error, Password, *};

/// Decompresses an archive file to a destination directory.
///
/// This is a convenience function for decompressing archive files directly from the filesystem.
///
/// # Arguments
/// * `src_path` - Path to the source archive file
/// * `dest` - Path to the destination directory where files will be extracted
pub fn decompress_file(src_path: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<(), Error> {
    let file = std::fs::File::open(src_path.as_ref())
        .map_err(|e| Error::file_open(e, src_path.as_ref().to_string_lossy().to_string()))?;
    decompress(file, dest)
}

/// Decompresses an archive file to a destination directory with a custom extraction function.
///
/// The extraction function is called for each entry in the archive, allowing custom handling
/// of individual files and directories during extraction.
///
/// # Arguments
/// * `src_path` - Path to the source archive file
/// * `dest` - Path to the destination directory where files will be extracted
/// * `extract_fn` - Custom function to handle each archive entry during extraction
pub fn decompress_file_with_extract_fn(
    src_path: impl AsRef<Path>,
    dest: impl AsRef<Path>,
    extract_fn: impl FnMut(&ArchiveEntry, &mut dyn Read, &PathBuf) -> Result<bool, Error>,
) -> Result<(), Error> {
    let file = std::fs::File::open(src_path.as_ref())
        .map_err(|e| Error::file_open(e, src_path.as_ref().to_string_lossy().to_string()))?;
    decompress_with_extract_fn(file, dest, extract_fn)
}

/// Decompresses an archive from a reader to a destination directory.
///
/// # Arguments
/// * `src_reader` - Reader containing the archive data
/// * `dest` - Path to the destination directory where files will be extracted
pub fn decompress<R: Read + Seek>(src_reader: R, dest: impl AsRef<Path>) -> Result<(), Error> {
    decompress_with_extract_fn(src_reader, dest, default_entry_extract_fn)
}

/// Decompresses an archive from a reader to a destination directory with a custom extraction function.
///
/// This provides the most flexibility, allowing both custom input sources and custom extraction logic.
///
/// # Arguments
/// * `src_reader` - Reader containing the archive data
/// * `dest` - Path to the destination directory where files will be extracted
/// * `extract_fn` - Custom function to handle each archive entry during extraction
#[cfg(not(target_arch = "wasm32"))]
pub fn decompress_with_extract_fn<R: Read + Seek>(
    src_reader: R,
    dest: impl AsRef<Path>,
    extract_fn: impl FnMut(&ArchiveEntry, &mut dyn Read, &PathBuf) -> Result<bool, Error>,
) -> Result<(), Error> {
    decompress_impl(src_reader, dest, Password::empty(), extract_fn)
}

/// Decompresses an encrypted archive file with the given password.
///
/// # Arguments
/// * `src_path` - Path to the encrypted source archive file
/// * `dest` - Path to the destination directory where files will be extracted
/// * `password` - Password to decrypt the archive
#[cfg(all(feature = "aes256", not(target_arch = "wasm32")))]
pub fn decompress_file_with_password(
    src_path: impl AsRef<Path>,
    dest: impl AsRef<Path>,
    password: Password,
) -> Result<(), Error> {
    let file = std::fs::File::open(src_path.as_ref())
        .map_err(|e| Error::file_open(e, src_path.as_ref().to_string_lossy().to_string()))?;
    decompress_with_password(file, dest, password)
}

/// Decompresses an encrypted archive from a reader with the given password.
///
/// # Arguments
/// * `src_reader` - Reader containing the encrypted archive data
/// * `dest` - Path to the destination directory where files will be extracted
/// * `password` - Password to decrypt the archive
#[cfg(all(feature = "aes256", not(target_arch = "wasm32")))]
pub fn decompress_with_password<R: Read + Seek>(
    src_reader: R,
    dest: impl AsRef<Path>,
    password: Password,
) -> Result<(), Error> {
    decompress_impl(src_reader, dest, password, default_entry_extract_fn)
}

/// Decompresses an encrypted archive from a reader with a custom extraction function and password.
///
/// This provides maximum flexibility for encrypted archives, allowing custom input sources,
/// custom extraction logic, and password decryption.
///
/// # Arguments
/// * `src_reader` - Reader containing the encrypted archive data
/// * `dest` - Path to the destination directory where files will be extracted
/// * `password` - Password to decrypt the archive
/// * `extract_fn` - Custom function to handle each archive entry during extraction
#[cfg(all(feature = "aes256", not(target_arch = "wasm32")))]
pub fn decompress_with_extract_fn_and_password<R: Read + Seek>(
    src_reader: R,
    dest: impl AsRef<Path>,
    password: Password,
    extract_fn: impl FnMut(&ArchiveEntry, &mut dyn Read, &PathBuf) -> Result<bool, Error>,
) -> Result<(), Error> {
    decompress_impl(src_reader, dest, password, extract_fn)
}

#[cfg(not(target_arch = "wasm32"))]
fn decompress_impl<R: Read + Seek>(
    mut src_reader: R,
    dest: impl AsRef<Path>,
    password: Password,
    mut extract_fn: impl FnMut(&ArchiveEntry, &mut dyn Read, &PathBuf) -> Result<bool, Error>,
) -> Result<(), Error> {
    use std::io::SeekFrom;

    let pos = src_reader.stream_position()?;
    src_reader.seek(SeekFrom::Start(pos))?;
    let mut seven = ArchiveReader::new(src_reader, password)?;
    let dest = PathBuf::from(dest.as_ref());
    if std::fs::symlink_metadata(&dest).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(Error::other("symlink extraction root"));
    }
    if !dest.exists() {
        std::fs::create_dir_all(&dest)?;
    }
    // Resolve user-selected aliases once (e.g. macOS /var -> /private/var).
    // All entry components below this root are checked without following links.
    let dest = std::fs::canonicalize(dest)?;
    seven.for_each_entries(|entry, reader| {
        let dest_path = safe_join(&dest, entry.name())?;
        reject_symlink_components(&dest_path)?;
        extract_fn(entry, reader, &dest_path)
    })?;

    Ok(())
}

/// Joins an untrusted archive entry name onto `dest`, rejecting any path that would
/// escape the destination directory (Zip-Slip / CWE-22).
///
/// Both `/` and `\` are treated as separators so Windows-style names are validated on
/// every platform, and any `..`, root, or drive-prefix component causes rejection.
#[cfg(not(target_arch = "wasm32"))]
fn safe_join(dest: &Path, entry_name: &str) -> Result<PathBuf, Error> {
    use std::path::Component;

    // Treat backslashes as separators too, so `..\..\x` from a Windows-authored
    // archive is caught when extracting on Unix.
    let normalized = entry_name.replace('\\', "/");
    if normalized.as_bytes().get(1) == Some(&b':') || normalized.contains('\0') {
        return Err(Error::other("unsafe archive entry name"));
    }
    let mut result = dest.to_path_buf();
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(part) => result.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::other(format!(
                    "unsafe entry path escapes destination: {entry_name}"
                )));
            }
        }
    }
    Ok(result)
}

#[cfg(not(target_arch = "wasm32"))]
fn reject_symlink_components(path: &Path) -> Result<(), Error> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(m) if m.file_type().is_symlink() => return Err(Error::other("symlink in extraction path")),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}

/// Default extraction function that handles standard file and directory extraction.
///
/// # Security
/// `dest` must already be a path you have validated as staying inside your destination
/// directory. This function rejects `..` and symlink path components; Unix writes
/// resolve ancestors through directory descriptors with `O_NOFOLLOW`. Because it
/// receives the already-joined path it cannot detect an absolute entry name
/// that discarded the destination root (`root.join("/etc/x")` == `/etc/x`), so do not
/// build `dest` with `root.join(entry.name())`. Prefer the higher-level [`decompress`]
/// helpers, which perform the full traversal check for you.
///
/// # Arguments
/// * `entry` - Archive entry being processed
/// * `reader` - Reader for the entry's data
/// * `dest` - Destination path for the entry (already validated by the caller)
#[cfg(not(target_arch = "wasm32"))]
pub fn default_entry_extract_fn(
    entry: &ArchiveEntry,
    reader: &mut dyn Read,
    dest: &PathBuf,
) -> Result<bool, Error> {
    use std::path::Component;

    // Reject any `..` component so a relative-traversal path can never reach a write.
    // (An absolute escape cannot be detected here, as the destination root is unknown.)
    if dest.components().any(|c| c == Component::ParentDir) {
        return Err(Error::other(format!(
            "unsafe entry path contains a parent-directory component: {}",
            dest.to_string_lossy()
        )));
    }

    #[cfg(unix)]
    {
        // Directory descriptors bind every component before opening the next;
        // ancestor replacements cannot redirect a later pathname write.
        return unix_extract(entry, reader, dest).map(|_| true).map_err(Error::from);
    }
    #[cfg(not(unix))]
    {
    reject_symlink_components(dest)?;
    if entry.is_directory() {
        let dir = dest;
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
        }
    } else {
        let path = dest;
        path.parent().and_then(|p| {
            if !p.exists() {
                std::fs::create_dir_all(p).ok()
            } else {
                None
            }
        });
        reject_symlink_components(path)?;
        let temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
        let file = temporary.as_file();
        if entry.size() > 0 {
            use std::io::Write;
            let mut writer = std::io::BufWriter::new(file);
            if std::io::copy(reader, &mut writer)? != entry.size() {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
            }
            writer.flush()?;

            let file = writer.get_mut();
            let file_times = FileTimes::new()
                .set_accessed(entry.access_date().into())
                .set_modified(entry.last_modified_date().into());

            #[cfg(any(windows, target_os = "macos"))]
            let file_times = file_times.set_created(entry.creation_date().into());

            let _ = file.set_times(file_times);
        }
        temporary.persist(path).map_err(|e| Error::from(e.error))?;
    }

    Ok(true)
    }
}

#[cfg(unix)]
fn unix_extract(entry: &ArchiveEntry, reader: &mut dyn Read, dest: &Path) -> std::io::Result<()> {
    use rustix::fs::{self, AtFlags, Mode, OFlags};
    use std::{fs::File, io::{self, Write}, path::Component, sync::atomic::{AtomicU64, Ordering}};
    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let directory = if entry.is_directory() { dest } else { dest.parent().unwrap_or(Path::new(".")) };
    let mut parent = File::from(fs::open(if directory.is_absolute() { "/" } else { "." }, dir_flags, Mode::empty())?);
    for component in directory.components() {
        let name = match component {
            Component::Normal(name) => name,
            Component::RootDir | Component::CurDir => continue,
            _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "unsafe directory component")),
        };
        match fs::openat(&parent, name, dir_flags, Mode::empty()) {
            Ok(fd) => parent = File::from(fd),
            Err(rustix::io::Errno::NOENT) => {
                match fs::mkdirat(&parent, name, Mode::from_bits_truncate(0o755)) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {},
                    Err(e) => return Err(e.into()),
                }
                parent = File::from(fs::openat(&parent, name, dir_flags, Mode::empty())?);
            },
            Err(e) => return Err(e.into()),
        }
    }
    if entry.is_directory() { return Ok(()); }
    let leaf = dest.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing output name"))?;

    // The staging directory is private and exclusively created. Publication is
    // relative to its descriptor and the already-bound destination parent.
    // A hostile writable parent can cause a collision or rename the directory,
    // but cannot make us follow a link or substitute the staged file.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let mut staging = None;
    for _ in 0..64 {
        let name = format!(".osz-7z-{}-{stamp}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        match fs::mkdirat(&parent, &name, Mode::from_bits_truncate(0o700)) {
            Ok(()) => {
                let dir = File::from(fs::openat(&parent, &name, dir_flags, Mode::empty())?);
                staging = Some((name, dir));
                break;
            },
            Err(rustix::io::Errno::EXIST) => {},
            Err(e) => return Err(e.into()),
        }
    }
    let (name, dir) = staging.ok_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "cannot reserve extraction staging directory"))?;
    let result = (|| -> io::Result<()> {
        let fd = fs::openat(&dir, "entry", OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::from_bits_truncate(0o600))?;
        let mut file = File::from(fd);
        let mut crc = crc32fast::Hasher::new();
        let mut written = 0u64;
        let mut buffer = [0u8; 64 << 10];
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 { break; }
            if n as u64 > entry.size().saturating_sub(written) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "entry exceeds declared size"));
            }
            file.write_all(&buffer[..n])?;
            crc.update(&buffer[..n]);
            written += n as u64;
        }
        if written != entry.size() { return Err(io::Error::from(io::ErrorKind::UnexpectedEof)); }
        if entry.has_crc && crc.finalize() as u64 != entry.crc {
            return Err(io::Error::other(Error::ChecksumVerificationFailed));
        }
        let times = FileTimes::new().set_accessed(entry.access_date().into()).set_modified(entry.last_modified_date().into());
        #[cfg(target_os = "macos")]
        let times = times.set_created(entry.creation_date().into());
        let _ = file.set_times(times);
        file.sync_all()?;
        fs::renameat(&dir, "entry", &parent, leaf)?;
        Ok(())
    })();
    let _ = fs::unlinkat(&dir, "entry", AtFlags::empty());
    let _ = fs::unlinkat(&parent, &name, AtFlags::REMOVEDIR);
    result
}
