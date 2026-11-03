//! Portable, bounded folder bundles. No symlinks, hard links, device nodes,
//! executable modes, absolute paths, or overwriting extraction destinations.
use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, Metadata, OpenOptions},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tempfile::NamedTempFile;
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

pub const MAX_BUNDLE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_ENTRIES: u32 = 10_000;
pub const MAX_DEPTH: usize = 32;
const MAX_PATH: usize = 4096;
const CHUNK: usize = 64 * 1024;
const MAGIC: &[u8; 9] = b"ASSIFOLD\x01";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FolderError {
    #[error("Операция с папкой отменена")]
    Cancelled,
    #[error("Недопустимое имя или путь внутри папки")]
    InvalidPath,
    #[error("В папке есть совпадающие имена или файл вместо каталога")]
    Collision,
    #[error("Символические и жёсткие ссылки, специальные файлы не поддерживаются")]
    UnsupportedEntry,
    #[error("Папка превышает лимит: 1 ГиБ, 10 000 элементов или 32 уровня")]
    Limit,
    #[error("Не удалось прочитать папку или сохранить её содержимое")]
    Io,
    #[error("Содержимое папки изменилось во время подготовки")]
    SourceChanged,
    #[error("Пакет папки повреждён или имеет неизвестный формат")]
    Corrupt,
    #[error("Не удалось создать новую папку без перезаписи существующей")]
    Publish,
}
pub type Result<T> = std::result::Result<T, FolderError>;

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(FolderError::Cancelled)
        } else {
            Ok(())
        }
    }
}

pub struct PreparedFolder {
    archive: NamedTempFile,
    pub name: String,
    pub entries: u32,
    /// Sum of regular file payloads, excluding the bounded bundle overhead.
    pub total_bytes: u64,
}
impl PreparedFolder {
    pub fn path(&self) -> &Path {
        self.archive.path()
    }
}

/// Fully validated private extraction, not yet published to its final directory.
/// Drop removes it. The caller can acquire its session/lifecycle lock, recheck
/// permission, then publish without holding the lock during bulk disk I/O.
pub struct PreparedExtraction {
    temporary: PrivateDirectory,
    parent: PathBuf,
    parent_dir: Dir,
    name: String,
    cancel: CancellationToken,
    pub entries: u32,
    pub total_bytes: u64,
}
impl PreparedExtraction {
    pub fn publish(mut self) -> Result<PathBuf> {
        self.cancel.check()?;
        let actual_parent =
            directory_path(&self.parent_dir, &self.parent).map_err(|_| FolderError::Publish)?;
        let from = std::ffi::OsStr::new(&self.temporary.name);
        for attempt in 0..100 {
            self.cancel.check()?;
            let name = if attempt == 0 {
                self.name.clone()
            } else {
                format!("{} ({})", short_prefix(&self.name, 200), attempt + 1)
            };
            match rename_new(&self.parent_dir, from, Path::new(&name), &self.parent) {
                Ok(()) => {
                    let destination = actual_parent.join(name);
                    self.temporary.published = true;
                    return Ok(destination);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(FolderError::Publish),
            }
        }
        Err(FolderError::Publish)
    }
}

/// All creation/cleanup is handle-relative, including after parent path replacement.
struct PrivateDirectory {
    directory: Option<Dir>,
    name: String,
    published: bool,
}
impl PrivateDirectory {
    fn new(parent: &Dir) -> Result<Self> {
        let parent = parent.try_clone().map_err(|_| FolderError::Io)?;
        let name = format!(".assi-receiving-{}", uuid::Uuid::new_v4().simple());
        let builder = cap_std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use cap_std::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        parent
            .create_dir_with(&name, &builder)
            .map_err(|_| FolderError::Io)?;
        Ok(Self {
            directory: Some(
                parent
                    .open_dir_nofollow(&name)
                    .map_err(|_| FolderError::Io)?,
            ),
            name,
            published: false,
        })
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        if !self.published {
            if let Some(directory) = self.directory.take() {
                let _ = directory.remove_open_dir_all();
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn directory_path(dir: &Dir, _fallback: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let path = rustix::fs::getpath(dir)?;
    Ok(std::ffi::OsString::from_vec(path.into_bytes()).into())
}
#[cfg(any(target_os = "linux", target_os = "android"))]
fn directory_path(dir: &Dir, fallback: &Path) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    if let Ok(path) = std::fs::read_link(format!("/proc/self/fd/{}", dir.as_raw_fd())) {
        return Ok(path);
    }
    let current = Dir::open_ambient_dir(fallback, ambient_authority())?.dir_metadata()?;
    let original = dir.dir_metadata()?;
    if current.dev() != original.dev() || current.ino() != original.ino() {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    Ok(fallback.into())
}
#[cfg(target_os = "windows")]
fn directory_path(dir: &Dir, _fallback: &Path) -> std::io::Result<PathBuf> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut buffer = vec![0u16; 32768];
    // SAFETY: The directory handle and writable UTF-16 buffer remain live.
    let count = unsafe {
        GetFinalPathNameByHandleW(
            dir.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            0,
        )
    } as usize;
    if count == 0 || count >= buffer.len() {
        return Err(std::io::Error::last_os_error());
    }
    let path = &buffer[..count];
    // Keep the caller-facing DOS/UNC spelling without lossy Unicode conversion.
    let prefix = [92u16, 92, 63, 92];
    let normalized = if path.starts_with(&prefix) {
        if path[4..].starts_with(&[85, 78, 67, 92]) {
            let mut value = vec![92, 92];
            value.extend_from_slice(&path[8..]);
            value
        } else {
            path[4..].to_vec()
        }
    } else {
        path.to_vec()
    };
    Ok(std::ffi::OsString::from_wide(&normalized).into())
}
#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "android",
    target_os = "windows"
)))]
fn directory_path(_dir: &Dir, _fallback: &Path) -> std::io::Result<PathBuf> {
    Err(std::io::ErrorKind::Unsupported.into())
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "android"))]
fn rename_new(
    parent: &Dir,
    from: &std::ffi::OsStr,
    to: &Path,
    _absolute: &Path,
) -> std::io::Result<()> {
    rustix::fs::renameat_with(parent, from, parent, to, rustix::fs::RenameFlags::NOREPLACE)
        .map_err(Into::into)
}
#[cfg(target_os = "windows")]
fn rename_new(
    parent: &Dir,
    from: &std::ffi::OsStr,
    to: &Path,
    _absolute: &Path,
) -> std::io::Result<()> {
    use cap_fs_ext::OpenOptionsMaybeDirExt;
    use cap_std::fs::OpenOptionsExt;
    use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, SetFileInformationByHandle, DELETE, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO,
        SYNCHRONIZE,
    };
    let mut options = OpenOptions::new();
    options
        .read(true)
        .access_mode(DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .maybe_dir(true)
        .follow(FollowSymlinks::No);
    let source = parent.open_with(from, &options)?;
    if !source.metadata()?.is_dir() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let name: Vec<u16> = to.as_os_str().encode_wide().collect();
    let name_offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let bytes = (name_offset + (name.len() + 1) * 2).max(std::mem::size_of::<FILE_RENAME_INFO>());
    // usize backing supplies FILE_RENAME_INFO alignment on both Windows targets.
    let mut buffer = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
    let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: The aligned buffer holds the structure and complete UTF-16 name;
    // both handles remain alive. RootDirectory binds the target to the captured
    // parent, and ReplaceIfExists=false forbids overwriting files or directories.
    unsafe {
        (*information).Anonymous.ReplaceIfExists = false;
        (*information).RootDirectory = parent.as_raw_handle();
        (*information).FileNameLength = (name.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            buffer
                .as_mut_ptr()
                .cast::<u8>()
                .add(name_offset)
                .cast::<u16>(),
            name.len(),
        );
    }
    // SAFETY: information points to the initialized buffer described above.
    if unsafe {
        SetFileInformationByHandle(
            source.as_raw_handle(),
            FileRenameInfo,
            information.cast(),
            bytes as u32,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "android",
    target_os = "windows"
)))]
fn rename_new(_: &Dir, _: &std::ffi::OsStr, _: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

fn short_prefix(name: &str, bytes: usize) -> String {
    let mut result = String::new();
    for c in name.chars() {
        if result.len() + c.len_utf8() > bytes {
            break;
        }
        result.push(c);
    }
    result
}
fn component(name: &str) -> Result<()> {
    if name.is_empty()||name.len()>255||matches!(name,"."|"..")||name.ends_with(['.',' '])
        || name.chars().any(|c|c.is_control()||matches!(c,'/'|'\\'|':'|'<'|'>'|'"'|'|'|'?'|'*'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')){return Err(FolderError::InvalidPath);}
    let normalized: String = name.nfkc().collect();
    let stem = normalized
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches([' ', '.'])
        .to_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && stem.as_bytes()[3].is_ascii_digit())
    {
        return Err(FolderError::InvalidPath);
    }
    Ok(())
}
fn relative(path: &str) -> Result<Vec<&str>> {
    if path.is_empty() || path.len() > MAX_PATH {
        return Err(FolderError::InvalidPath);
    }
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() > MAX_DEPTH {
        return Err(FolderError::Limit);
    }
    for part in &parts {
        component(part)?;
    }
    Ok(parts)
}
fn collision_key(path: &str) -> String {
    path.nfc()
        .flat_map(char::to_uppercase)
        .collect::<String>()
        .nfc()
        .collect()
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Directory = 1,
    File = 2,
}
fn insert_path(seen: &mut HashMap<String, (String, Kind)>, path: &str, kind: Kind) -> Result<()> {
    relative(path)?;
    let key = collision_key(path);
    if seen.contains_key(&key) {
        return Err(FolderError::Collision);
    }
    if let Some((parent, _)) = path.rsplit_once('/') {
        if !seen
            .get(&collision_key(parent))
            .is_some_and(|(original, kind)| original == parent && *kind == Kind::Directory)
        {
            return Err(FolderError::Collision);
        }
    }
    seen.insert(key, (path.into(), kind));
    Ok(())
}

struct Entry {
    path: String,
    kind: Kind,
    size: u64,
    identity: Option<Metadata>,
}
fn plain_file(metadata: &Metadata) -> Result<()> {
    if !metadata.is_file() || metadata.is_symlink() || metadata.nlink() != 1 {
        Err(FolderError::UnsupportedEntry)
    } else {
        Ok(())
    }
}
fn open_file(dir: &Dir, name: &str) -> Result<cap_std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_fs_ext::OpenOptionsSyncExt;
        options.nonblock(true);
    }
    let file = dir
        .open_with(name, &options)
        .map_err(|_| FolderError::UnsupportedEntry)?;
    plain_file(&file.metadata().map_err(|_| FolderError::Io)?)?;
    Ok(file)
}
fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.modified().ok() == b.modified().ok()
}
fn walk(
    dir: &Dir,
    prefix: &str,
    entries: &mut Vec<Entry>,
    seen: &mut HashMap<String, (String, Kind)>,
    bytes: &mut u64,
    cancel: &CancellationToken,
) -> Result<()> {
    cancel.check()?;
    let mut names = Vec::new();
    for entry in dir.entries().map_err(|_| FolderError::Io)? {
        cancel.check()?;
        if names.len() + entries.len() >= MAX_ENTRIES as usize {
            return Err(FolderError::Limit);
        }
        let name = entry
            .map_err(|_| FolderError::Io)?
            .file_name()
            .into_string()
            .map_err(|_| FolderError::InvalidPath)?;
        component(&name)?;
        names.push(name);
    }
    names.sort();
    for name in names {
        cancel.check()?;
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        relative(&path)?;
        if entries.len() >= MAX_ENTRIES as usize {
            return Err(FolderError::Limit);
        }
        let metadata = dir.symlink_metadata(&name).map_err(|_| FolderError::Io)?;
        if metadata.is_symlink() {
            return Err(FolderError::UnsupportedEntry);
        }
        if metadata.is_dir() {
            let child = dir
                .open_dir_nofollow(&name)
                .map_err(|_| FolderError::UnsupportedEntry)?;
            insert_path(seen, &path, Kind::Directory)?;
            entries.push(Entry {
                path: path.clone(),
                kind: Kind::Directory,
                size: 0,
                identity: None,
            });
            walk(&child, &path, entries, seen, bytes, cancel)?;
        } else {
            if !metadata.is_file() {
                return Err(FolderError::UnsupportedEntry);
            }
            let file = open_file(dir, &name)?;
            let metadata = file.metadata().map_err(|_| FolderError::Io)?;
            *bytes = bytes
                .checked_add(metadata.len())
                .ok_or(FolderError::Limit)?;
            if *bytes > MAX_BUNDLE_BYTES {
                return Err(FolderError::Limit);
            }
            insert_path(seen, &path, Kind::File)?;
            entries.push(Entry {
                path,
                kind: Kind::File,
                size: metadata.len(),
                identity: Some(metadata),
            });
        }
    }
    Ok(())
}
fn directory_for(root: &Dir, path: &str) -> Result<(Dir, String)> {
    let parts = relative(path)?;
    let mut dir = root.try_clone().map_err(|_| FolderError::Io)?;
    for name in &parts[..parts.len() - 1] {
        dir = dir
            .open_dir_nofollow(name)
            .map_err(|_| FolderError::UnsupportedEntry)?;
    }
    Ok((dir, parts[parts.len() - 1].into()))
}
fn source_dir(source: &Path) -> Result<(Dir, String)> {
    let name = source
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(FolderError::InvalidPath)?
        .to_owned();
    component(&name)?;
    let parent = source
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = Dir::open_ambient_dir(parent, ambient_authority()).map_err(|_| FolderError::Io)?;
    let root = parent
        .open_dir_nofollow(&name)
        .map_err(|_| FolderError::UnsupportedEntry)?;
    Ok((root, name))
}

pub fn pack(source: &Path) -> Result<PreparedFolder> {
    pack_with_cancel(source, &CancellationToken::default())
}
pub fn pack_with_cancel(source: &Path, cancel: &CancellationToken) -> Result<PreparedFolder> {
    cancel.check()?;
    let (root, name) = source_dir(source)?;
    let mut entries = Vec::new();
    let mut total = 0;
    let mut seen = HashMap::new();
    walk(&root, "", &mut entries, &mut seen, &mut total, cancel)?;
    let overhead = 9
        + 2
        + name.len() as u64
        + 4
        + 8
        + 32
        + entries
            .iter()
            .map(|e| 11 + e.path.len() as u64)
            .sum::<u64>();
    if total + overhead > MAX_BUNDLE_BYTES {
        return Err(FolderError::Limit);
    }
    let prefix = format!("{}-", short_prefix(&name, 120));
    let mut archive = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".assi-folder")
        .tempfile()
        .map_err(|_| FolderError::Io)?;
    let mut digest = Sha256::new();
    let write = |file: &mut File, digest: &mut Sha256, bytes: &[u8]| -> Result<()> {
        file.write_all(bytes).map_err(|_| FolderError::Io)?;
        digest.update(bytes);
        Ok(())
    };
    write(archive.as_file_mut(), &mut digest, MAGIC)?;
    write(
        archive.as_file_mut(),
        &mut digest,
        &(name.len() as u16).to_be_bytes(),
    )?;
    write(archive.as_file_mut(), &mut digest, name.as_bytes())?;
    write(
        archive.as_file_mut(),
        &mut digest,
        &(entries.len() as u32).to_be_bytes(),
    )?;
    write(archive.as_file_mut(), &mut digest, &total.to_be_bytes())?;
    let mut buffer = [0u8; CHUNK];
    for entry in &entries {
        cancel.check()?;
        write(archive.as_file_mut(), &mut digest, &[entry.kind as u8])?;
        write(
            archive.as_file_mut(),
            &mut digest,
            &(entry.path.len() as u16).to_be_bytes(),
        )?;
        write(
            archive.as_file_mut(),
            &mut digest,
            &entry.size.to_be_bytes(),
        )?;
        write(archive.as_file_mut(), &mut digest, entry.path.as_bytes())?;
        if entry.kind == Kind::Directory {
            continue;
        }
        let (parent, name) = directory_for(&root, &entry.path)?;
        let mut input = open_file(&parent, &name)?;
        let expected = entry.identity.as_ref().ok_or(FolderError::SourceChanged)?;
        if !same_file(expected, &input.metadata().map_err(|_| FolderError::Io)?) {
            return Err(FolderError::SourceChanged);
        }
        let mut remaining = entry.size;
        while remaining > 0 {
            cancel.check()?;
            let count = remaining.min(CHUNK as u64) as usize;
            input
                .read_exact(&mut buffer[..count])
                .map_err(|_| FolderError::SourceChanged)?;
            write(archive.as_file_mut(), &mut digest, &buffer[..count])?;
            remaining -= count as u64;
        }
        let current = input.metadata().map_err(|_| FolderError::Io)?;
        plain_file(&current)?;
        if !same_file(expected, &current)
            || input.read(&mut buffer[..1]).map_err(|_| FolderError::Io)? != 0
        {
            return Err(FolderError::SourceChanged);
        }
    }
    cancel.check()?;
    archive
        .write_all(&digest.finalize())
        .map_err(|_| FolderError::Io)?;
    archive.as_file().sync_all().map_err(|_| FolderError::Io)?;
    archive
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|_| FolderError::Io)?;
    Ok(PreparedFolder {
        archive,
        name,
        entries: entries.len() as u32,
        total_bytes: total,
    })
}

struct BundleReader {
    file: File,
    digest: Sha256,
    consumed: u64,
    length: u64,
}
impl BundleReader {
    fn bytes(&mut self, len: usize) -> Result<Vec<u8>> {
        let mut bytes = vec![0; len];
        self.read(&mut bytes)?;
        Ok(bytes)
    }
    fn read(&mut self, bytes: &mut [u8]) -> Result<()> {
        let next = self
            .consumed
            .checked_add(bytes.len() as u64)
            .ok_or(FolderError::Limit)?;
        if next > self.length.saturating_sub(32) {
            return Err(FolderError::Corrupt);
        }
        self.file
            .read_exact(bytes)
            .map_err(|_| FolderError::Corrupt)?;
        self.digest.update(bytes);
        self.consumed = next;
        Ok(())
    }
    fn text(&mut self, max: usize) -> Result<String> {
        let mut size = [0; 2];
        self.read(&mut size)?;
        let length = usize::from(u16::from_be_bytes(size));
        if length == 0 || length > max {
            return Err(FolderError::InvalidPath);
        }
        String::from_utf8(self.bytes(length)?).map_err(|_| FolderError::InvalidPath)
    }
    fn finish(mut self) -> Result<()> {
        if self.consumed + 32 != self.length {
            return Err(FolderError::Corrupt);
        }
        let mut expected = [0; 32];
        self.file
            .read_exact(&mut expected)
            .map_err(|_| FolderError::Corrupt)?;
        let actual: [u8; 32] = self.digest.finalize().into();
        if actual != expected {
            return Err(FolderError::Corrupt);
        }
        let mut extra = [0];
        if self.file.read(&mut extra).map_err(|_| FolderError::Io)? != 0 {
            return Err(FolderError::Corrupt);
        }
        Ok(())
    }
}
pub fn unpack(bundle: &Path, parent: &Path) -> Result<PathBuf> {
    prepare_unpack(bundle, parent, &CancellationToken::default())?.publish()
}
pub fn prepare_unpack(
    bundle: &Path,
    parent: &Path,
    cancel: &CancellationToken,
) -> Result<PreparedExtraction> {
    cancel.check()?;
    let bundle_parent = Dir::open_ambient_dir(
        bundle
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
        ambient_authority(),
    )
    .map_err(|_| FolderError::Io)?;
    let bundle_name = bundle
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(FolderError::InvalidPath)?;
    let file = open_file(&bundle_parent, bundle_name)?.into_std();
    let length = file.metadata().map_err(|_| FolderError::Io)?.len();
    if length > MAX_BUNDLE_BYTES {
        return Err(FolderError::Limit);
    }
    if length < 56 {
        return Err(FolderError::Corrupt);
    }
    let mut input = BundleReader {
        file,
        digest: Sha256::new(),
        consumed: 0,
        length,
    };
    if input.bytes(MAGIC.len())? != MAGIC {
        return Err(FolderError::Corrupt);
    }
    let name = input.text(255)?;
    component(&name)?;
    let mut count = [0; 4];
    input.read(&mut count)?;
    let count = u32::from_be_bytes(count);
    if count > MAX_ENTRIES {
        return Err(FolderError::Limit);
    }
    let mut total = [0; 8];
    input.read(&mut total)?;
    let total = u64::from_be_bytes(total);
    if total > MAX_BUNDLE_BYTES || total > length {
        return Err(FolderError::Limit);
    }
    let parent_dir =
        Dir::open_ambient_dir(parent, ambient_authority()).map_err(|_| FolderError::Io)?;
    let temporary = PrivateDirectory::new(&parent_dir)?;
    let dir = temporary
        .directory
        .as_ref()
        .ok_or(FolderError::Io)?
        .try_clone()
        .map_err(|_| FolderError::Io)?;
    let mut seen = HashMap::new();
    let mut actual = 0u64;
    let mut buffer = [0u8; CHUNK];
    for _ in 0..count {
        cancel.check()?;
        let mut header = [0; 11];
        input.read(&mut header)?;
        let kind = match header[0] {
            1 => Kind::Directory,
            2 => Kind::File,
            _ => return Err(FolderError::Corrupt),
        };
        let path_len = usize::from(u16::from_be_bytes([header[1], header[2]]));
        if path_len == 0 || path_len > MAX_PATH {
            return Err(FolderError::InvalidPath);
        }
        let size = u64::from_be_bytes(header[3..].try_into().map_err(|_| FolderError::Corrupt)?);
        if (kind == Kind::Directory && size != 0) || size > total.saturating_sub(actual) {
            return Err(FolderError::Corrupt);
        }
        let path =
            String::from_utf8(input.bytes(path_len)?).map_err(|_| FolderError::InvalidPath)?;
        insert_path(&mut seen, &path, kind)?;
        let (parent, name) = directory_for(&dir, &path)?;
        if kind == Kind::Directory {
            parent.create_dir(name).map_err(|_| FolderError::Io)?;
            continue;
        }
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = parent
            .open_with(name, &options)
            .map_err(|_| FolderError::Io)?;
        let mut remaining = size;
        while remaining > 0 {
            cancel.check()?;
            let count = remaining.min(CHUNK as u64) as usize;
            input.read(&mut buffer[..count])?;
            output
                .write_all(&buffer[..count])
                .map_err(|_| FolderError::Io)?;
            remaining -= count as u64;
        }
        output.sync_all().map_err(|_| FolderError::Io)?;
        actual += size;
    }
    if actual != total {
        return Err(FolderError::Corrupt);
    }
    input.finish()?;
    cancel.check()?;
    Ok(PreparedExtraction {
        temporary,
        parent: parent.into(),
        parent_dir,
        name,
        cancel: cancel.clone(),
        entries: count,
        total_bytes: total,
    })
}

#[cfg(test)]
mod tests;
