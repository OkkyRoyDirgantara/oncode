//! File access for the explorer and editor: folder listing, open/save text, and file operations.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};
use serde::{Deserialize, Serialize};

use crate::error::{Result, WsError, WsErrorCode};
use crate::gitignore::IgnoreRules;

/// Number of leading bytes inspected to detect a binary file.
const BINARY_SNIFF_BYTES: usize = 8000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    /// Absolute path.
    pub path: String,
    /// For symlinks, follows the kind of the target.
    pub is_dir: bool,
    pub is_symlink: bool,
    /// Ignored by gitignore rules; shown dimmed in the explorer.
    pub ignored: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Eol {
    Lf,
    Crlf,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    /// Size in bytes; capped at `u32::MAX`.
    pub size: u32,
    /// Modification time, milliseconds since the Unix epoch.
    pub mtime_ms: f64,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFile {
    /// The file contents; line endings are already normalized to `\n`.
    pub text: String,
    /// Encoding name per `encoding_rs`, e.g. `UTF-8`, `windows-1252`, `UTF-16LE`.
    pub encoding: String,
    pub bom: bool,
    pub eol: Eol,
    pub stat: FileStat,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OpenedFile {
    Text(TextFile),
    Binary { size: u32 },
    TooLarge { size: u32 },
}

/// The contents of `dir` (must be inside `root`): folders first, then files, each
/// sorted case-insensitively. `.git` entries are not included.
///
/// `ignored` is true if the entry is ignored by the gitignore rules that apply under
/// `root` (nested `.gitignore`, `.git/info/exclude`, and the global gitignore).
pub async fn read_dir(root: &Path, dir: &Path) -> Result<Vec<DirEntry>> {
    let root = absolute(root)?;
    let dir = if dir.is_absolute() { normalize(dir) } else { normalize(&root.join(dir)) };
    if !dir.starts_with(&root) {
        return Err(WsError::invalid_path(format!("Folder is outside the project: {}", dir.display())));
    }
    blocking(move || read_dir_sync(&root, &dir)).await
}

fn read_dir_sync(root: &Path, dir: &Path) -> Result<Vec<DirEntry>> {
    let rules = IgnoreRules::new(root);
    // The contents of an ignored folder are ignored entirely.
    let parent_ignored = rules.is_ignored(dir, true);
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| WsError::io(dir, e))? {
        let entry = entry.map_err(|e| WsError::io(dir, e))?;
        let file_name = entry.file_name();
        if file_name == ".git" {
            continue;
        }
        let path = dir.join(&file_name);
        let Ok(file_type) = entry.file_type() else { continue };
        let is_symlink = file_type.is_symlink();
        let is_dir = if is_symlink { fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) } else { file_type.is_dir() };
        // Git treats symlinks as files, so a `name/` pattern does not match them.
        let ignored = parent_ignored || rules.is_entry_ignored(dir, &path, file_type.is_dir());
        entries.push(DirEntry {
            name: file_name.to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            is_dir,
            is_symlink,
            ignored,
        });
    }
    entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase(), e.name.clone()));
    Ok(entries)
}

/// Opens a file for the editor.
///
/// - `TooLarge` if its size exceeds `max_bytes` (the contents are not read).
/// - `Binary` if there is a NUL byte in the first 8000 bytes, unless the file starts with a UTF-16 BOM.
/// - Encoding: BOM first (UTF-8, UTF-16LE, UTF-16BE); without a BOM, valid UTF-8
///   is treated as `UTF-8`; otherwise it is guessed by `chardetng`.
/// - `eol` is `Crlf` if `\r\n` outnumbers lone `\n`.
pub async fn open_file(path: &Path, max_bytes: u64) -> Result<OpenedFile> {
    let path = path.to_path_buf();
    blocking(move || open_file_sync(&path, max_bytes, None)).await
}

/// Like [`open_file`], but the text is decoded with `encoding` (a WHATWG label, e.g. `windows-1252`)
/// instead of detecting it: "Reopen with Encoding". A BOM of that encoding is still recognized.
pub async fn open_file_with_encoding(path: &Path, max_bytes: u64, encoding: &str) -> Result<OpenedFile> {
    let forced = Encoding::for_label(encoding.as_bytes()).ok_or_else(|| WsError::other(format!("Unknown encoding: {encoding}")))?;
    let path = path.to_path_buf();
    blocking(move || open_file_sync(&path, max_bytes, Some(forced))).await
}

fn open_file_sync(path: &Path, max_bytes: u64, forced: Option<&'static Encoding>) -> Result<OpenedFile> {
    let meta = fs::metadata(path).map_err(|e| WsError::io(path, e))?;
    if !meta.is_file() {
        return Err(WsError::invalid_path(format!("Not a regular file: {}", path.display())));
    }
    if meta.len() > max_bytes {
        return Ok(OpenedFile::TooLarge { size: saturate(meta.len()) });
    }
    let bytes = fs::read(path).map_err(|e| WsError::io(path, e))?;
    let size = bytes.len() as u64;
    if size > max_bytes {
        return Ok(OpenedFile::TooLarge { size: saturate(size) });
    }
    let decoded = match forced {
        Some(encoding) => Some(decode_as(bytes, encoding)),
        None => decode(bytes),
    };
    match decoded {
        None => Ok(OpenedFile::Binary { size: saturate(size) }),
        Some(decoded) => {
            let mut stat = stat_of(&meta);
            stat.size = saturate(size);
            Ok(OpenedFile::Text(TextFile {
                text: decoded.text,
                encoding: decoded.encoding.name().to_string(),
                bom: decoded.bom,
                eol: decoded.eol,
                stat,
            }))
        }
    }
}

/// Reads a whole file as bytes (e.g. an image to show). Fails if it is larger than `max_bytes`.
pub async fn read_bytes(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let path = path.to_path_buf();
    blocking(move || {
        let meta = fs::metadata(&path).map_err(|e| WsError::io(&path, e))?;
        if !meta.is_file() {
            return Err(WsError::invalid_path(format!("Not a regular file: {}", path.display())));
        }
        if meta.len() > max_bytes {
            return Err(WsError::other(format!("File is too large to read: {}", path.display())));
        }
        fs::read(&path).map_err(|e| WsError::io(&path, e))
    })
    .await
}

struct Decoded {
    text: String,
    encoding: &'static Encoding,
    bom: bool,
    eol: Eol,
}

/// `None` means the contents are considered binary.
fn decode(bytes: Vec<u8>) -> Option<Decoded> {
    let bom = Encoding::for_bom(&bytes);
    let is_utf16 = matches!(bom, Some((encoding, _)) if encoding == UTF_16LE || encoding == UTF_16BE);
    if !is_utf16 && bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return None;
    }
    let (encoding, text) = match bom {
        Some((encoding, bom_len)) => (encoding, encoding.decode_without_bom_handling(&bytes[bom_len..]).0.into_owned()),
        None => match String::from_utf8(bytes) {
            Ok(text) => (UTF_8, text),
            Err(err) => {
                let bytes = err.into_bytes();
                let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
                detector.feed(&bytes, true);
                let encoding = detector.guess(None, chardetng::Utf8Detection::Deny);
                (encoding, encoding.decode_without_bom_handling(&bytes).0.into_owned())
            }
        },
    };
    let (text, eol) = normalize_eol(text);
    Some(Decoded { text, encoding, bom: bom.is_some(), eol })
}

/// Decodes with a chosen encoding; a leading BOM of that encoding is dropped (and remembered).
fn decode_as(bytes: Vec<u8>, encoding: &'static Encoding) -> Decoded {
    let bom = Encoding::for_bom(&bytes).filter(|(found, _)| *found == encoding);
    let start = bom.map_or(0, |(_, length)| length);
    let text = encoding.decode_without_bom_handling(&bytes[start..]).0.into_owned();
    let (text, eol) = normalize_eol(text);
    Decoded { text, encoding, bom: bom.is_some(), eol }
}

/// Converts `\r\n` and lone `\r` into `\n`, and determines the dominant line ending.
fn normalize_eol(text: String) -> (String, Eol) {
    if !text.contains('\r') {
        return (text, Eol::Lf);
    }
    let mut out = String::with_capacity(text.len());
    let (mut crlf, mut lf) = (0usize, 0usize);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    crlf += 1;
                }
                out.push('\n');
            }
            '\n' => {
                lf += 1;
                out.push('\n');
            }
            other => out.push(other),
        }
    }
    (out, if crlf > lf { Eol::Crlf } else { Eol::Lf })
}

/// Saves text atomically (a temporary file in the same folder, then rename) while
/// preserving the permissions of an existing file.
///
/// `text` uses `\n`; line endings are converted to `eol`, then the text is encoded with `encoding`
/// (a WHATWG label, e.g. `UTF-8`, `windows-1252`, `UTF-16LE`) and prefixed with a BOM if `bom`
/// (only applies to UTF-8 and UTF-16). Characters that do not exist in `encoding`
/// produce an `Io` error; the file on disk is not touched. A read-only file is rejected with
/// `PermissionDenied`.
pub async fn save_file(path: &Path, text: &str, encoding: &str, bom: bool, eol: Eol) -> Result<FileStat> {
    let bytes = encode(text, encoding, bom, eol)?;
    let path = path.to_path_buf();
    blocking(move || {
        write_atomic(&path, &bytes)?;
        let meta = fs::metadata(&path).map_err(|e| WsError::io(&path, e))?;
        Ok(stat_of(&meta))
    })
    .await
}

fn encode(text: &str, label: &str, bom: bool, eol: Eol) -> Result<Vec<u8>> {
    let encoding = Encoding::for_label(label.trim().as_bytes())
        .ok_or_else(|| WsError::other(format!("Unknown encoding: {label}")))?;

    let mut normalized = if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n") } else { text.to_owned() };
    if eol == Eol::Crlf {
        normalized = normalized.replace('\n', "\r\n");
    }

    let mut out = Vec::with_capacity(normalized.len() + 3);
    if encoding == UTF_16LE || encoding == UTF_16BE {
        // `encoding_rs` does not provide a UTF-16 encoder, so it is done directly.
        let little = encoding == UTF_16LE;
        if bom {
            out.extend_from_slice(if little { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
        }
        for unit in normalized.encode_utf16() {
            out.extend_from_slice(&if little { unit.to_le_bytes() } else { unit.to_be_bytes() });
        }
        return Ok(out);
    }

    let (bytes, used, unmappable) = encoding.encode(&normalized);
    if unmappable {
        return Err(WsError::other(format!(
            "The text contains characters that cannot be saved with the {} encoding. Save as UTF-8 or remove those characters.",
            encoding.name()
        )));
    }
    if bom && used == UTF_8 {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    out.extend_from_slice(&bytes);
    Ok(out)
}

static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);

const TEMP_PREFIX: &str = ".oncode-";
const TEMP_SUFFIX: &str = ".tmp";

/// True for the temporary file names created by [`write_atomic`]; the watcher ignores them.
pub(crate) fn is_temp_name(name: &str) -> bool {
    name.starts_with(TEMP_PREFIX) && name.ends_with(TEMP_SUFFIX)
}

/// Removes the temporary file if the write fails midway.
struct TempGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn create_temp(dir: &Path) -> Result<(TempGuard, fs::File)> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    for _ in 0..16 {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("{TEMP_PREFIX}{:x}-{nanos:x}-{n:x}{TEMP_SUFFIX}", std::process::id());
        let path = dir.join(name);
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((TempGuard { path, armed: true }, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(WsError::io(dir, err)),
        }
    }
    Err(WsError::other(format!("Cannot create a temporary file in {}", dir.display())))
}

/// Replaces the contents of `path` atomically. Symlinks are followed so the link stays intact.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let target = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    };
    let existing = fs::metadata(&target).ok();
    if let Some(meta) = &existing {
        if meta.is_dir() {
            return Err(WsError::invalid_path(format!("Not a file: {}", target.display())));
        }
        if meta.permissions().readonly() {
            return Err(WsError::new(
                WsErrorCode::PermissionDenied,
                format!("File is read-only and cannot be saved: {}", target.display()),
            ));
        }
    }
    let dir = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };

    let (mut guard, mut file) = create_temp(dir)?;
    file.write_all(bytes).map_err(|e| WsError::io(&target, e))?;
    file.sync_all().map_err(|e| WsError::io(&target, e))?;
    drop(file);
    if let Some(meta) = &existing {
        fs::set_permissions(&guard.path, meta.permissions()).map_err(|e| WsError::io(&target, e))?;
    }
    fs::rename(&guard.path, &target).map_err(|e| WsError::io(&target, e))?;
    guard.armed = false;
    Ok(())
}

/// File metadata; `None` if the path does not exist.
pub async fn stat(path: &Path) -> Result<Option<FileStat>> {
    let path = path.to_path_buf();
    blocking(move || match fs::metadata(&path) {
        Ok(meta) => Ok(Some(stat_of(&meta))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(WsError::io(&path, err)),
    })
    .await
}

/// Creates an empty file along with its parent folders. `AlreadyExists` if the path already exists.
pub async fn create_file(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    blocking(move || {
        if fs::symlink_metadata(&path).is_ok() {
            return Err(WsError::already_exists(&path));
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| WsError::io(parent, e))?;
        }
        fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| WsError::io(&path, e))?;
        Ok(())
    })
    .await
}

/// Creates a folder along with its parent folders. `AlreadyExists` if the path already exists.
pub async fn create_dir(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    blocking(move || {
        if fs::symlink_metadata(&path).is_ok() {
            return Err(WsError::already_exists(&path));
        }
        fs::create_dir_all(&path).map_err(|e| WsError::io(&path, e))
    })
    .await
}

/// Renames or moves. Never overwrites: `AlreadyExists` if `to` already exists.
/// The parent folder of `to` is created if it does not exist.
pub async fn rename(from: &Path, to: &Path) -> Result<()> {
    let from = from.to_path_buf();
    let to = to.to_path_buf();
    blocking(move || {
        if fs::symlink_metadata(&from).is_err() {
            return Err(WsError::not_found(&from));
        }
        if fs::symlink_metadata(&to).is_ok() && !is_same_entry(&from, &to) {
            return Err(WsError::already_exists(&to));
        }
        if let Some(parent) = to.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| WsError::io(parent, e))?;
        }
        fs::rename(&from, &to).map_err(|e| WsError::io(&to, e))
    })
    .await
}

/// Copies files or folders (along with their contents) into the folder `dest_dir`, e.g. the result of a drag-and-drop
/// from outside the project. The sources are unrestricted; confining to the project is up to the caller, via `dest_dir`.
///
/// Never overwrites: if any name already exists in `dest_dir`, or two sources have the
/// same name, nothing is copied and the result is `AlreadyExists`. A folder cannot be copied into
/// itself. Inside a source folder, symlinks to files are copied as regular files; symlinks to
/// folders (and special files such as sockets) are skipped so the copy does not loop or escape
/// the source folder. If it fails midway, the copies already made are removed again.
///
/// Returns the paths of the copies, in the order of `sources`.
pub async fn copy_into(sources: &[PathBuf], dest_dir: &Path) -> Result<Vec<PathBuf>> {
    let sources = sources.to_vec();
    let dest_dir = dest_dir.to_path_buf();
    blocking(move || {
        match fs::metadata(&dest_dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(WsError::invalid_path(format!("Not a folder: {}", dest_dir.display()))),
            Err(err) => return Err(WsError::io(&dest_dir, err)),
        }
        let dest_real = fs::canonicalize(&dest_dir).map_err(|e| WsError::io(&dest_dir, e))?;

        // Everything is checked first, so there is no half-finished copy just because the second name collides.
        let mut targets: Vec<PathBuf> = Vec::with_capacity(sources.len());
        for source in &sources {
            let meta = fs::metadata(source).map_err(|e| WsError::io(source, e))?;
            if !meta.is_dir() && !meta.is_file() {
                return Err(WsError::invalid_path(format!("Only files and folders can be copied: {}", source.display())));
            }
            let name = source.file_name().ok_or_else(|| WsError::invalid_path(format!("Cannot copy: {}", source.display())))?;
            if meta.is_dir() && fs::canonicalize(source).is_ok_and(|real| dest_real.starts_with(&real)) {
                return Err(WsError::invalid_path(format!("A folder cannot be copied into itself: {}", source.display())));
            }
            let target = dest_dir.join(name);
            if fs::symlink_metadata(&target).is_ok() || targets.contains(&target) {
                return Err(WsError::already_exists(&target));
            }
            targets.push(target);
        }

        for (index, (source, target)) in sources.iter().zip(&targets).enumerate() {
            if let Err(err) = copy_tree(source, target) {
                // A name that someone else took in the meantime is not ours; do not delete it.
                let created = if err.code == WsErrorCode::AlreadyExists { index } else { index + 1 };
                for target in &targets[..created] {
                    let _ = if target.is_dir() { fs::remove_dir_all(target) } else { fs::remove_file(target) };
                }
                return Err(err);
            }
        }
        Ok(targets)
    })
    .await
}

/// Copies a single file or folder to a `target` that does not exist yet.
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    let meta = fs::metadata(source).map_err(|e| WsError::io(source, e))?;
    if meta.is_dir() {
        fs::create_dir(target).map_err(|e| WsError::io(target, e))?;
        for entry in fs::read_dir(source).map_err(|e| WsError::io(source, e))? {
            let entry = entry.map_err(|e| WsError::io(source, e))?;
            let path = entry.path();
            let is_link = entry.file_type().map_err(|e| WsError::io(&path, e))?.is_symlink();
            if is_link && !fs::metadata(&path).is_ok_and(|meta| meta.is_file()) {
                continue;
            }
            copy_tree(&path, &target.join(entry.file_name()))?;
        }
        Ok(())
    } else if meta.is_file() {
        // `create_new`: never overwrites, even if the name was only taken after the check.
        let mut input = fs::File::open(source).map_err(|e| WsError::io(source, e))?;
        let mut output = fs::OpenOptions::new().write(true).create_new(true).open(target).map_err(|e| WsError::io(target, e))?;
        std::io::copy(&mut input, &mut output).map_err(|e| WsError::io(target, e))?;
        // Permissions (e.g. the executable bit) are copied too, like `fs::copy`.
        let _ = fs::set_permissions(target, meta.permissions());
        Ok(())
    } else {
        Ok(())
    }
}

/// True if both paths point to the same entry and differ only in case
/// (renaming `readme.md` → `README.md` on a case-insensitive file system).
fn is_same_entry(from: &Path, to: &Path) -> bool {
    if from == to {
        return true;
    }
    let differs_only_by_case = from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase();
    differs_only_by_case
        && match (fs::canonicalize(from), fs::canonicalize(to)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// Moves files or folders to the operating system's trash.
pub async fn trash(paths: &[PathBuf]) -> Result<()> {
    // Nothing to delete: succeeds on every OS (the `trash` library rejects an empty list on Windows).
    if paths.is_empty() {
        return Ok(());
    }
    let paths = paths.to_vec();
    blocking(move || {
        for path in &paths {
            if fs::symlink_metadata(path).is_err() {
                return Err(WsError::not_found(path));
            }
        }
        ::trash::delete_all(&paths).map_err(|e| WsError::other(format!("Failed to move to trash: {e}")))
    })
    .await
}

/// True if `path` is inside `root` (or equal to `root`).
///
/// `..` and `.` are resolved lexically, so the path does not have to exist. Relative paths
/// are interpreted against `root`. Symlinks are not followed.
pub fn is_inside(root: &Path, path: &Path) -> bool {
    let root = normalize(root);
    let path = if path.is_absolute() { normalize(path) } else { normalize(&root.join(path)) };
    path.starts_with(&root)
}

/// Resolves `.` and `..` without touching the file system.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// An absolute path that has been lexically normalized.
pub(crate) fn absolute(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path).map_err(|e| WsError::io(path, e))?;
    Ok(normalize(&absolute))
}

fn stat_of(meta: &fs::Metadata) -> FileStat {
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    FileStat { size: saturate(meta.len()), mtime_ms, read_only: meta.permissions().readonly() }
}

pub(crate) fn saturate(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

async fn blocking<T, F>(work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(err) => Err(WsError::other(format!("Background task failed: {err}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: impl AsRef<[u8]>) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn text_of(opened: OpenedFile) -> TextFile {
        match opened {
            OpenedFile::Text(text) => text,
            other => panic!("not text: {other:?}"),
        }
    }

    const MAX: u64 = 1 << 20;

    // ---- read_dir ----

    #[tokio::test]
    async fn read_dir_orders_dirs_first_and_flags_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join(".git/info/exclude"), "secret.txt\n");
        write(&root.join(".gitignore"), "*.log\ntarget/\n");
        write(&root.join("b.txt"), "");
        write(&root.join("A.txt"), "");
        write(&root.join("c.log"), "");
        write(&root.join("secret.txt"), "");
        write(&root.join("zeta/x"), "");
        write(&root.join("Beta/x"), "");
        write(&root.join("alpha/x"), "");
        write(&root.join("target/debug/x"), "");
        write(&root.join("alpha/.gitignore"), "/local\n");
        write(&root.join("alpha/local"), "");
        write(&root.join("alpha/y.log"), "");

        let entries = read_dir(root, root).await.unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Beta", "target", "zeta", ".gitignore", "A.txt", "b.txt", "c.log", "secret.txt"]);
        let ignored: Vec<&str> = entries.iter().filter(|e| e.ignored).map(|e| e.name.as_str()).collect();
        assert_eq!(ignored, ["target", "c.log", "secret.txt"]);
        assert!(entries.iter().take(4).all(|e| e.is_dir));
        assert!(entries.iter().skip(4).all(|e| !e.is_dir));
        assert!(entries.iter().all(|e| Path::new(&e.path).is_absolute()));
        assert_eq!(Path::new(&entries[0].path), root.join("alpha"));

        // Nested .gitignore.
        let alpha = read_dir(root, &root.join("alpha")).await.unwrap();
        let flags: Vec<(&str, bool)> = alpha.iter().map(|e| (e.name.as_str(), e.ignored)).collect();
        assert_eq!(flags, [(".gitignore", false), ("local", true), ("x", false), ("y.log", true)]);

        // The contents of an ignored folder are ignored too.
        let target = read_dir(root, &root.join("target")).await.unwrap();
        assert_eq!(target.len(), 1);
        assert!(target[0].ignored && target[0].is_dir);
    }

    #[tokio::test]
    async fn read_dir_rejects_outside_and_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        let err = read_dir(&root, tmp.path()).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::InvalidPath);
        let err = read_dir(&root, &root.join("sub/../../other")).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::InvalidPath);
        let err = read_dir(&root, &root.join("nope")).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::NotFound);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_dir_reports_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("real/x"), "");
        write(&root.join("file.txt"), "");
        std::os::unix::fs::symlink(root.join("real"), root.join("link-dir")).unwrap();
        std::os::unix::fs::symlink(root.join("file.txt"), root.join("link-file")).unwrap();
        std::os::unix::fs::symlink(root.join("gone"), root.join("dangling")).unwrap();
        let entries = read_dir(root, root).await.unwrap();
        let view: Vec<(&str, bool, bool)> = entries.iter().map(|e| (e.name.as_str(), e.is_dir, e.is_symlink)).collect();
        assert_eq!(
            view,
            [
                ("link-dir", true, true),
                ("real", true, false),
                ("dangling", false, true),
                ("file.txt", false, false),
                ("link-file", false, true)
            ]
        );
    }

    // ---- open_file ----

    #[tokio::test]
    async fn opens_utf8() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.txt");
        write(&path, "hello world\nsecond line é\n");
        let file = text_of(open_file(&path, MAX).await.unwrap());
        assert_eq!(file.text, "hello world\nsecond line é\n");
        assert_eq!(file.encoding, "UTF-8");
        assert!(!file.bom);
        assert_eq!(file.eol, Eol::Lf);
        assert_eq!(file.stat.size, 27);
        assert!(file.stat.mtime_ms > 1.0e12);
        assert!(!file.stat.read_only);
    }

    #[tokio::test]
    async fn opens_empty_file_as_text() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty");
        write(&path, "");
        let file = text_of(open_file(&path, MAX).await.unwrap());
        assert_eq!((file.text.as_str(), file.encoding.as_str(), file.bom, file.eol), ("", "UTF-8", false, Eol::Lf));
    }

    #[tokio::test]
    async fn opens_utf8_with_bom() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.txt");
        write(&path, b"\xEF\xBB\xBFhello\n");
        let file = text_of(open_file(&path, MAX).await.unwrap());
        assert_eq!(file.text, "hello\n");
        assert_eq!(file.encoding, "UTF-8");
        assert!(file.bom);
    }

    fn utf16(text: &str, little: bool, bom: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if bom {
            out.extend_from_slice(if little { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
        }
        for unit in text.encode_utf16() {
            out.extend_from_slice(&if little { unit.to_le_bytes() } else { unit.to_be_bytes() });
        }
        out
    }

    #[tokio::test]
    async fn opens_utf16_with_bom() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("le.txt");
        write(&path, utf16("hello é 😀\r\nline\r\n", true, true));
        let file = text_of(open_file(&path, MAX).await.unwrap());
        assert_eq!(file.text, "hello é 😀\nline\n");
        assert_eq!(file.encoding, "UTF-16LE");
        assert!(file.bom);
        assert_eq!(file.eol, Eol::Crlf);

        let path = tmp.path().join("be.txt");
        write(&path, utf16("hello\n", false, true));
        let file = text_of(open_file(&path, MAX).await.unwrap());
        assert_eq!(file.text, "hello\n");
        assert_eq!(file.encoding, "UTF-16BE");
        assert!(file.bom);
    }

    const FRENCH: &str = "Voilà une phrase entièrement rédigée en français, où l'élève a été très étonné par la beauté du château.\nÀ bientôt, cher ami : ça va être génial !\n";

    #[tokio::test]
    async fn opens_latin1_as_windows_1252() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("latin1.txt");
        let (bytes, _, unmappable) = encoding_rs::WINDOWS_1252.encode(FRENCH);
        assert!(!unmappable);
        assert!(std::str::from_utf8(&bytes).is_err());
        write(&path, &bytes);
        let file = text_of(open_file(&path, MAX).await.unwrap());
        assert_eq!(file.encoding, "windows-1252");
        assert_eq!(file.text, FRENCH);
        assert!(!file.bom);
    }

    /// Reopen with Encoding: valid UTF-8 read as windows-1252 on purpose, and an unknown label refused.
    #[tokio::test]
    async fn opens_with_a_chosen_encoding() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.txt");
        write(&path, "\u{feff}é\r\n");
        let file = text_of(open_file_with_encoding(&path, MAX, "windows-1252").await.unwrap());
        assert_eq!((file.encoding.as_str(), file.text.as_str(), file.bom, file.eol), ("windows-1252", "ï»¿Ã©\n", false, Eol::Crlf));
        let file = text_of(open_file_with_encoding(&path, MAX, "utf-8").await.unwrap());
        assert_eq!((file.text.as_str(), file.bom), ("é\n", true));
        assert!(open_file_with_encoding(&path, MAX, "nope").await.is_err());
    }

    #[tokio::test]
    async fn detects_line_endings() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("crlf.txt");
        write(&path, "a\r\nb\r\nc\n");
        let file = text_of(open_file(&path, MAX).await.unwrap());
        assert_eq!(file.eol, Eol::Crlf);
        assert_eq!(file.text, "a\nb\nc\n");

        // A tie: lone `\n` wins.
        write(&path, "a\r\nb\n");
        assert_eq!(text_of(open_file(&path, MAX).await.unwrap()).eol, Eol::Lf);

        // A lone `\r` is normalized to `\n`.
        write(&path, "a\rb\n");
        let file = text_of(open_file(&path, MAX).await.unwrap());
        assert_eq!((file.text.as_str(), file.eol), ("a\nb\n", Eol::Lf));
    }

    #[tokio::test]
    async fn reads_bytes_up_to_the_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("image.png");
        write(&path, b"\x89PNG\r\n\x1a\n\0\0");
        assert_eq!(read_bytes(&path, 10).await.unwrap(), b"\x89PNG\r\n\x1a\n\0\0");
        assert_eq!(read_bytes(&path, 9).await.unwrap_err().code, WsErrorCode::Io, "larger than the limit");
        assert_eq!(read_bytes(&tmp.path().join("missing.png"), 10).await.unwrap_err().code, WsErrorCode::NotFound);
        assert_eq!(read_bytes(tmp.path(), 10).await.unwrap_err().code, WsErrorCode::InvalidPath, "a folder");
    }

    #[tokio::test]
    async fn detects_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bin");
        write(&path, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR");
        assert_eq!(open_file(&path, MAX).await.unwrap(), OpenedFile::Binary { size: 16 });

        // A UTF-8 BOM does not exempt the NUL check.
        write(&path, b"\xEF\xBB\xBFab\0cd");
        assert_eq!(open_file(&path, MAX).await.unwrap(), OpenedFile::Binary { size: 8 });

        // A NUL after the first 8000 bytes does not count.
        let mut late = vec![b'a'; 9000];
        late.push(0);
        write(&path, &late);
        assert!(matches!(open_file(&path, MAX).await.unwrap(), OpenedFile::Text(_)));
    }

    #[tokio::test]
    async fn reports_too_large_missing_and_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("big.txt");
        write(&path, "x".repeat(101));
        assert_eq!(open_file(&path, 100).await.unwrap(), OpenedFile::TooLarge { size: 101 });
        assert!(matches!(open_file(&path, 101).await.unwrap(), OpenedFile::Text(_)));

        let err = open_file(&tmp.path().join("nope"), MAX).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::NotFound);
        let err = open_file(tmp.path(), MAX).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::InvalidPath);
    }

    #[test]
    fn opened_file_serializes_with_kind_tag() {
        let stat = FileStat { size: 3, mtime_ms: 1.5, read_only: false };
        let text = OpenedFile::Text(TextFile { text: "abc".into(), encoding: "UTF-8".into(), bom: false, eol: Eol::Crlf, stat });
        assert_eq!(
            serde_json::to_value(&text).unwrap(),
            serde_json::json!({
                "kind": "text", "text": "abc", "encoding": "UTF-8", "bom": false, "eol": "crlf",
                "stat": { "size": 3, "mtimeMs": 1.5, "readOnly": false }
            })
        );
        assert_eq!(
            serde_json::to_value(OpenedFile::TooLarge { size: 9 }).unwrap(),
            serde_json::json!({ "kind": "tooLarge", "size": 9 })
        );
        assert_eq!(serde_json::from_str::<Eol>("\"lf\"").unwrap(), Eol::Lf);
        let entry = DirEntry { name: "a".into(), path: "/a".into(), is_dir: true, is_symlink: false, ignored: false };
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            serde_json::json!({ "name": "a", "path": "/a", "isDir": true, "isSymlink": false, "ignored": false })
        );
    }

    // ---- save_file ----

    async fn round_trip(text: &str, encoding: &str, bom: bool, eol: Eol) -> Vec<u8> {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        let stat = save_file(&path, text, encoding, bom, eol).await.unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(stat.size as usize, bytes.len());
        let file = text_of(open_file(&path, MAX).await.unwrap());
        assert_eq!(file.text, text, "{encoding} bom={bom} {eol:?}");
        assert_eq!(file.encoding, encoding);
        assert_eq!(file.bom, bom);
        assert_eq!(file.eol, eol);
        assert_eq!(file.stat, stat);
        bytes
    }

    #[tokio::test]
    async fn save_round_trips_utf8() {
        let bytes = round_trip("one\ntwo é\n", "UTF-8", false, Eol::Lf).await;
        assert_eq!(bytes, "one\ntwo é\n".as_bytes());
        let bytes = round_trip("one\ntwo\n", "UTF-8", false, Eol::Crlf).await;
        assert_eq!(bytes, b"one\r\ntwo\r\n");
        let bytes = round_trip("one\ntwo\n", "UTF-8", true, Eol::Crlf).await;
        assert_eq!(bytes, b"\xEF\xBB\xBFone\r\ntwo\r\n");
    }

    #[tokio::test]
    async fn save_round_trips_utf16() {
        let text = "hello é 😀\nline\n";
        let bytes = round_trip(text, "UTF-16LE", true, Eol::Lf).await;
        assert_eq!(bytes, utf16(text, true, true));
        let bytes = round_trip(text, "UTF-16LE", true, Eol::Crlf).await;
        assert_eq!(bytes, utf16("hello é 😀\r\nline\r\n", true, true));
        let bytes = round_trip(text, "UTF-16BE", true, Eol::Crlf).await;
        assert_eq!(bytes, utf16("hello é 😀\r\nline\r\n", false, true));
    }

    #[tokio::test]
    async fn save_round_trips_windows_1252() {
        let bytes = round_trip(FRENCH, "windows-1252", false, Eol::Lf).await;
        assert_eq!(bytes, encoding_rs::WINDOWS_1252.encode(FRENCH).0.as_ref());
        let crlf = FRENCH.replace('\n', "\r\n");
        let bytes = round_trip(FRENCH, "windows-1252", false, Eol::Crlf).await;
        assert_eq!(bytes, encoding_rs::WINDOWS_1252.encode(&crlf).0.as_ref());
    }

    #[tokio::test]
    async fn save_rejects_unmappable_text_and_unknown_encoding() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        write(&path, "original");
        let err = save_file(&path, "emoji 😀", "windows-1252", false, Eol::Lf).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::Io);
        assert!(err.message.contains("windows-1252"));
        let err = save_file(&path, "x", "not-an-encoding", false, Eol::Lf).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::Io);
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1, "no temporary file left behind");
    }

    #[tokio::test]
    async fn save_never_doubles_carriage_returns() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        save_file(&path, "a\r\nb\n", "UTF-8", false, Eol::Crlf).await.unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"a\r\nb\r\n");
        save_file(&path, "a\r\nb\n", "UTF-8", false, Eol::Lf).await.unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"a\nb\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn save_replaces_atomically_and_keeps_permissions() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("run.sh");
        write(&path, "#!/bin/sh\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o754)).unwrap();
        let inode_before = fs::metadata(&path).unwrap().ino();

        let stat = save_file(&path, "#!/bin/sh\necho hey\n", "UTF-8", false, Eol::Lf).await.unwrap();
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o754);
        assert_ne!(meta.ino(), inode_before, "the file is replaced via rename, not written in place");
        assert_eq!(fs::read_to_string(&path).unwrap(), "#!/bin/sh\necho hey\n");
        assert_eq!(stat.size, 19);
        assert!(!stat.read_only);
        let names: Vec<_> = fs::read_dir(tmp.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["run.sh"], "no temporary file left behind");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn save_refuses_read_only_and_writes_through_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ro.txt");
        write(&path, "original");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let err = save_file(&path, "new", "UTF-8", false, Eol::Lf).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::PermissionDenied);
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert!(stat(&path).await.unwrap().unwrap().read_only);

        let target = tmp.path().join("target.txt");
        let link = tmp.path().join("link.txt");
        write(&target, "old");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        save_file(&link, "new", "UTF-8", false, Eol::Lf).await.unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    #[tokio::test]
    async fn save_creates_new_file_but_not_missing_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("new.txt");
        save_file(&path, "content\n", "utf-8", false, Eol::Lf).await.unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"content\n");
        #[cfg(unix)]
        {
            // A new file gets the default permissions (following the umask), not the temporary file's permissions.
            use std::os::unix::fs::PermissionsExt;
            let plain = tmp.path().join("plain.txt");
            fs::write(&plain, "x").unwrap();
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), mode(&plain));
        }
        let err = save_file(&tmp.path().join("not/there/x.txt"), "content", "UTF-8", false, Eol::Lf).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::NotFound);
    }

    // ---- stat / create / rename ----

    #[tokio::test]
    async fn stat_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(stat(&tmp.path().join("nope")).await.unwrap(), None);
        write(&tmp.path().join("a"), "12345");
        let found = stat(&tmp.path().join("a")).await.unwrap().unwrap();
        assert_eq!(found.size, 5);
        assert!(found.mtime_ms > 1.0e12);
    }

    #[tokio::test]
    async fn create_file_and_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a/b/c.txt");
        create_file(&file).await.unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"");
        assert_eq!(create_file(&file).await.unwrap_err().code, WsErrorCode::AlreadyExists);
        assert_eq!(create_file(&tmp.path().join("a")).await.unwrap_err().code, WsErrorCode::AlreadyExists);

        let dir = tmp.path().join("x/y");
        create_dir(&dir).await.unwrap();
        assert!(dir.is_dir());
        assert_eq!(create_dir(&dir).await.unwrap_err().code, WsErrorCode::AlreadyExists);
        assert_eq!(create_dir(&file).await.unwrap_err().code, WsErrorCode::AlreadyExists);
    }

    #[tokio::test]
    async fn rename_moves_but_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        write(&a, "A");
        write(&b, "B");
        let err = rename(&a, &b).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::AlreadyExists);
        assert_eq!(fs::read(&a).unwrap(), b"A");
        assert_eq!(fs::read(&b).unwrap(), b"B");

        let c = tmp.path().join("sub/dir/c.txt");
        rename(&a, &c).await.unwrap();
        assert!(!a.exists());
        assert_eq!(fs::read(&c).unwrap(), b"A");

        // A folder along with its contents.
        rename(&tmp.path().join("sub"), &tmp.path().join("moved")).await.unwrap();
        assert_eq!(fs::read(tmp.path().join("moved/dir/c.txt")).unwrap(), b"A");
        fs::create_dir(tmp.path().join("other")).unwrap();
        let err = rename(&tmp.path().join("moved"), &tmp.path().join("other")).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::AlreadyExists);

        let err = rename(&tmp.path().join("nope"), &tmp.path().join("x")).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::NotFound);
    }

    #[tokio::test]
    async fn copy_into_copies_files_and_folders_with_their_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        let project = tmp.path().join("project");
        write(&outside.join("a.txt"), "A");
        write(&outside.join("tree/one.txt"), "1");
        write(&outside.join("tree/deep/two.txt"), "2");
        fs::create_dir_all(outside.join("tree/empty")).unwrap();
        fs::create_dir_all(project.join("dest")).unwrap();

        let copied = copy_into(&[outside.join("a.txt"), outside.join("tree")], &project.join("dest")).await.unwrap();
        assert_eq!(copied, [project.join("dest/a.txt"), project.join("dest/tree")]);
        assert_eq!(fs::read(project.join("dest/a.txt")).unwrap(), b"A");
        assert_eq!(fs::read(project.join("dest/tree/one.txt")).unwrap(), b"1");
        assert_eq!(fs::read(project.join("dest/tree/deep/two.txt")).unwrap(), b"2");
        assert!(project.join("dest/tree/empty").is_dir());
        // The source is untouched.
        assert_eq!(fs::read(outside.join("tree/one.txt")).unwrap(), b"1");

        // Nothing to copy: an empty list simply succeeds.
        assert!(copy_into(&[], &project.join("dest")).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn copy_into_never_overwrites_and_copies_nothing_when_a_name_is_taken() {
        let tmp = tempfile::tempdir().unwrap();
        let (source, dest) = (tmp.path().join("src"), tmp.path().join("dest"));
        write(&source.join("new.txt"), "new");
        write(&source.join("taken.txt"), "from outside");
        write(&dest.join("taken.txt"), "original");

        // The first file is free, but the second collides: the first file is not copied either.
        let err = copy_into(&[source.join("new.txt"), source.join("taken.txt")], &dest).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::AlreadyExists);
        assert!(!dest.join("new.txt").exists());
        assert_eq!(fs::read(dest.join("taken.txt")).unwrap(), b"original");

        // Two sources with the same name from different folders also collide with each other.
        write(&tmp.path().join("other/new.txt"), "different");
        let err = copy_into(&[source.join("new.txt"), tmp.path().join("other/new.txt")], &dest).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::AlreadyExists);
        assert!(!dest.join("new.txt").exists());
    }

    #[tokio::test]
    async fn copy_into_rejects_missing_sources_bad_destinations_and_copying_a_folder_into_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&dest).unwrap();
        write(&tmp.path().join("file.txt"), "x");
        write(&tmp.path().join("tree/inner/leaf.txt"), "x");

        let err = copy_into(&[tmp.path().join("nope")], &dest).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::NotFound);
        let err = copy_into(&[tmp.path().join("file.txt")], &tmp.path().join("missing")).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::NotFound);
        let err = copy_into(&[tmp.path().join("file.txt")], &tmp.path().join("file.txt")).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::InvalidPath);

        // Into itself or its descendants would loop endlessly.
        let err = copy_into(&[tmp.path().join("tree")], &tmp.path().join("tree")).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::InvalidPath);
        let err = copy_into(&[tmp.path().join("tree")], &tmp.path().join("tree/inner")).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::InvalidPath);
        assert!(!tmp.path().join("tree/inner/tree").exists());
        // Copying into its own parent is not a folder-into-itself problem, but an already existing name.
        let err = copy_into(&[tmp.path().join("tree")], tmp.path()).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::AlreadyExists);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copy_into_follows_file_links_but_skips_folder_links_inside_a_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("tree");
        write(&tree.join("real.txt"), "content");
        std::os::unix::fs::symlink(tree.join("real.txt"), tree.join("file-link")).unwrap();
        // A link to its parent folder would make the copy loop if followed.
        std::os::unix::fs::symlink(&tree, tree.join("loop")).unwrap();
        std::os::unix::fs::symlink(tree.join("nowhere"), tree.join("broken")).unwrap();
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&dest).unwrap();

        copy_into(&[tree], &dest).await.unwrap();
        assert_eq!(fs::read(dest.join("tree/file-link")).unwrap(), b"content");
        assert!(!fs::symlink_metadata(dest.join("tree/file-link")).unwrap().file_type().is_symlink(), "copied as a regular file");
        assert!(!dest.join("tree/loop").exists() && !dest.join("tree/broken").exists());
    }

    #[tokio::test]
    async fn trash_reports_missing_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let err = trash(&[tmp.path().join("nope")]).await.unwrap_err();
        assert_eq!(err.code, WsErrorCode::NotFound);
        trash(&[]).await.unwrap();
    }

    /// Moves real files to the user's trash, so it is not run by default:
    /// `cargo test -p oncode-workspace -- --ignored trash_moves`.
    #[tokio::test]
    #[ignore]
    async fn trash_moves_files_and_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("oncode-trash-test.txt");
        let dir = tmp.path().join("oncode-trash-test-dir");
        write(&file, "x");
        write(&dir.join("inner.txt"), "x");
        trash(&[file.clone(), dir.clone()]).await.unwrap();
        assert!(!file.exists());
        assert!(!dir.exists());
    }

    // ---- is_inside ----

    #[test]
    fn is_inside_resolves_dots_lexically() {
        let root = Path::new("/home/u/proj");
        assert!(is_inside(root, Path::new("/home/u/proj")));
        assert!(is_inside(root, Path::new("/home/u/proj/src/main.rs")));
        assert!(is_inside(root, Path::new("/home/u/proj/src/../Cargo.toml")));
        assert!(is_inside(root, Path::new("/home/u/proj/./a/./b")));
        assert!(is_inside(root, Path::new("/home/u/other/../proj/a")));
        assert!(!is_inside(root, Path::new("/home/u/proj/../secret")));
        assert!(!is_inside(root, Path::new("/home/u/proj/a/../../secret")));
        assert!(!is_inside(root, Path::new("/home/u/project")));
        assert!(!is_inside(root, Path::new("/home/u")));
        assert!(!is_inside(root, Path::new("/")));
        assert!(!is_inside(root, Path::new("/../../etc/passwd")));
        // Relative paths are interpreted against root.
        assert!(is_inside(root, Path::new("src/lib.rs")));
        assert!(is_inside(root, Path::new("a/../b")));
        assert!(!is_inside(root, Path::new("../proj2/x")));
        assert!(!is_inside(root, Path::new("a/../../x")));
        // A root that has not been normalized.
        assert!(is_inside(Path::new("/home/u/x/../proj/"), Path::new("/home/u/proj/a")));
    }

    #[test]
    fn normalize_handles_edges() {
        assert_eq!(normalize(Path::new("/a/b/../c/./d")), Path::new("/a/c/d"));
        assert_eq!(normalize(Path::new("/..")), Path::new("/"));
        assert_eq!(normalize(Path::new("a/../../b")), Path::new("../b"));
        assert_eq!(normalize(Path::new("./a")), Path::new("a"));
    }
}
