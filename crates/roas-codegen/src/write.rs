//! Turning a [`Generation`] into files, safely: see [`write()`].

use crate::config::Target;
use crate::generate::Generation;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// How a [`write()`] treats the output directory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WriteOptions {
    /// Names this generation target, together with the language. Two
    /// targets writing to one directory need different labels, and their
    /// generated files need different names.
    pub label: String,
    /// Overwrite an existing file that no manifest owns. Off by default:
    /// such a file is probably hand-written. One that already holds exactly
    /// the generated content is adopted either way, since nothing is lost.
    pub overwrite_unowned: bool,
    /// Write a generation that carries `Error` diagnostics. Off by
    /// default: such a generation is missing the types the errors
    /// suppressed. When it writes such a generation, files the previous run
    /// wrote are kept rather than removed as stale; a clean generation
    /// removes them as usual.
    pub allow_partial: bool,
    /// What this label was generated from, recorded in the manifest. When
    /// set, a later run under the same label from a different source is
    /// refused, so two descriptions cannot overwrite each other's output.
    /// The `roas` command line uses the description's path relative to
    /// the output directory, without a `.json`/`.yaml`/`.yml` extension,
    /// so moving between JSON and YAML keeps the label. An empty id counts
    /// as none, and switches the check off: use a non-empty one, such as
    /// `.`, for a source that has no shorter name.
    pub source_id: Option<String>,
    /// Accept a different [`source_id`](Self::source_id) than the one the
    /// label recorded, and record the new one: the description moved or was
    /// renamed. Off by default, so a second description cannot take over a
    /// label by accident.
    pub replace_source: bool,
}

impl WriteOptions {
    pub fn new(label: impl Into<String>) -> Self {
        WriteOptions {
            label: label.into(),
            overwrite_unowned: false,
            allow_partial: false,
            source_id: None,
            replace_source: false,
        }
    }
}

/// What a [`write()`] did, path by path, relative to the output directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WriteReport {
    /// Created or changed.
    pub written: Vec<PathBuf>,
    /// Already held exactly the generated content; left untouched.
    pub unchanged: Vec<PathBuf>,
    /// Renamed to a new spelling of the same name (old, new), as on a
    /// case-insensitive disk; also listed as written or unchanged.
    pub renamed: Vec<(PathBuf, PathBuf)>,
    /// Owned by the previous run, no longer generated, and removed.
    pub removed: Vec<PathBuf>,
    /// Owned by the previous run and no longer generated, but edited since,
    /// so kept on disk and dropped from the manifest.
    pub kept_edited: Vec<PathBuf>,
    /// The manifest, relative to the output directory.
    pub manifest: PathBuf,
}

/// What a [`check()`] found, path by path, relative to the output directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckReport {
    /// On disk, but different from what would be generated, or stored
    /// under another spelling of its name.
    pub changed: Vec<PathBuf>,
    /// Would be generated, but absent.
    pub missing: Vec<PathBuf>,
    /// Owned by the previous run and no longer generated: a write removes
    /// them.
    pub stale: Vec<PathBuf>,
    /// No longer generated, but edited since: a write keeps them and stops
    /// tracking them.
    pub stale_edited: Vec<PathBuf>,
    /// The manifest is absent, or differs from the one a [`write()`]
    /// would leave in anything but the writer's version.
    pub manifest_outdated: bool,
}

impl CheckReport {
    /// True when the directory already holds exactly this generation.
    pub fn is_clean(&self) -> bool {
        self.changed.is_empty()
            && self.missing.is_empty()
            && self.stale.is_empty()
            && self.stale_edited.is_empty()
            && !self.manifest_outdated
    }
}

/// Why the output directory could not be written or checked.
///
/// Paths of generated files and directories are relative to the output
/// directory: in `UnsafePath`, `NonAsciiPath`, `ConflictingPaths`,
/// `ReservedPath`, `Unowned`, `OwnedByOther`, `IsADirectory`,
/// `DirectorySpelling`, and `Symlink`/`NotADirectory`. A manifest in
/// `OwnedByOther` or `OutputDirOwned` is relative to it too when it sits
/// below it, and named as found when it sits above. The rest name the path
/// as opened.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WriteError {
    #[error("label {0:?} must be non-empty and use only lowercase letters, digits, `-` and `_`")]
    InvalidLabel(String),
    #[error("generated path {0} must be relative and stay inside the output directory")]
    UnsafePath(PathBuf),
    #[error(
        "generated path {0} must be ASCII: disks disagree on which other spellings name the same file"
    )]
    NonAsciiPath(PathBuf),
    #[error(
        "{path} is owned by another generation target (manifest {manifest}); use a different output directory, or a label or module name that generates a different file; if that target is this description under an old label, reuse that label or delete its manifest"
    )]
    OwnedByOther { path: PathBuf, manifest: PathBuf },
    #[error(
        "{0} already exists and was not written by this generator; move it, or allow overwriting unowned files"
    )]
    Unowned(PathBuf),
    #[error("generated path {0} is reserved for the generator's own manifest and temporary files")]
    ReservedPath(PathBuf),
    #[error("{0} is in the way: a generated path needs it to be a directory")]
    NotADirectory(PathBuf),
    #[error("{0} is a directory where a generated file goes")]
    IsADirectory(PathBuf),
    #[error("{0} must be a regular file: the generator takes its lock on it")]
    BadLock(PathBuf),
    #[error(
        "output directory {0} is a symbolic link; generated files are never written through one"
    )]
    OutputDirSymlink(PathBuf),
    #[error("output directory {0} is not a directory")]
    OutputDirNotADirectory(PathBuf),
    #[error(
        "the output directory is, or is inside, a file another generation target owns (manifest {manifest}); use a different output directory"
    )]
    OutputDirOwned { manifest: PathBuf },
    #[error(
        "directory {stored} is spelled {planned} by this generation: the manifest would record a path a case-sensitive checkout does not have; rename the directory, or keep its spelling"
    )]
    DirectorySpelling { planned: PathBuf, stored: PathBuf },
    #[error("{0} is a symbolic link; generated files are never written through one")]
    Symlink(PathBuf),
    #[error(
        "{0} is locked by another generation run in this directory or an overlapping one; retry when it finishes"
    )]
    Locked(PathBuf),
    #[error(
        "generated paths {0} and {1} collide: the same file ignoring case, or a file where the other needs a directory"
    )]
    ConflictingPaths(PathBuf, PathBuf),
    #[error(
        "label {label:?} belongs to output generated from {previous:?}, not {current:?}; if it is a different description, give it its own label; if the description or the output directory moved, allow replacing the recorded source or delete the label's manifest"
    )]
    SourceMismatch {
        label: String,
        previous: String,
        current: String,
    },
    #[error(
        "the generation has {0} error diagnostic(s); refused (allow a partial generation to accept it)"
    )]
    HasErrors(usize),
    #[error("manifest {path} cannot be read: {reason}")]
    Manifest { path: PathBuf, reason: String },
    #[error(
        "{manifest} is another Go generation target's manifest, and Go files in one directory are one package, where both would declare the same helpers; give each description its own directory, or delete that manifest and the files it lists if the target is gone"
    )]
    SharedGoPackage { manifest: PathBuf },
    #[error("cannot access {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The manifest's on-disk form. `files` maps a relative path, written
/// with `/`, to the hash of the content this target wrote there.
// Unknown fields are tolerated so that a manifest a newer version writes
// does not stop an older one from working in the same directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Manifest {
    generator: String,
    /// The roas-codegen version that wrote it. Informational: it is not
    /// compared, so a patch upgrade with identical output changes nothing.
    version: String,
    label: String,
    language: String,
    /// [`WriteOptions::source_id`], when the caller set one.
    #[serde(default)]
    source: String,
    files: BTreeMap<String, String>,
    /// The hash each file had before the run that wrote this manifest
    /// started replacing it. Only a pending manifest has entries: if that
    /// run is interrupted, a file may still hold its old content, which is
    /// as much ours as the new.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    replacing: BTreeMap<String, String>,
}

impl Manifest {
    /// Equal in everything that matters: all but the writer's version.
    fn same_as(&self, other: &Manifest) -> bool {
        self.generator == other.generator
            && self.label == other.label
            && self.language == other.language
            && self.source == other.source
            && self.files == other.files
            && self.replacing == other.replacing
    }

    /// Whether `content` is something this manifest wrote at `path`.
    fn wrote(&self, path: &str, content: &[u8]) -> bool {
        let actual = hash(content);
        self.files.get(path) == Some(&actual) || self.replacing.get(path) == Some(&actual)
    }
}

const MANIFEST_PREFIX: &str = ".roas-codegen-";
const MANIFEST_SUFFIX: &str = ".json";
const TMP_MARKER: &str = ".roas-tmp-";
const LOCK_FILE: &str = ".roas-codegen.lock";

/// An operating-system lock on a directory's `.roas-codegen.lock`, held
/// for one run. It ends with the process, so a killed run never blocks the
/// next. A lock file is only ever removed by the run that created it, while
/// holding it, when that run is refused before writing anything; otherwise it stays, since its
/// presence tells a run with an overlapping output root that this
/// directory is one.
struct DirLock {
    /// Held for the lock; released when dropped.
    _file: fs::File,
    /// Set when this run created the file.
    #[cfg_attr(not(unix), allow(dead_code))]
    made: Option<PathBuf>,
}

impl Drop for DirLock {
    fn drop(&mut self) {
        // Unlocked explicitly, not only by closing: a child process another
        // thread forks meanwhile shares the open file until it execs, and
        // closing our copy alone would leave the lock held by it.
        let _ = self._file.unlock();
    }
}

impl DirLock {
    /// Exclusive, on this run's own output directory, creating the file.
    fn acquire(out_dir: &Path) -> Result<DirLock, WriteError> {
        check_lock(out_dir)?;
        let path = out_dir.join(LOCK_FILE);
        let open = |new: bool| {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(new)
                .open(&path)
        };
        let (file, made) = match open(true) {
            Ok(file) => (file, true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Read-write where possible: some filesystems (NFS on
                // Linux) give an exclusive lock only to a writable handle.
                // Read-only otherwise, which local filesystems accept.
                let file = match open(false) {
                    Ok(file) => file,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::PermissionDenied
                                | std::io::ErrorKind::ReadOnlyFilesystem
                        ) =>
                    {
                        fs::File::open(&path).map_err(io(&path))?
                    }
                    // Removed in between by a refused run that made it:
                    // another run is active here.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(WriteError::Locked(path));
                    }
                    Err(source) => return Err(WriteError::Io { path, source }),
                };
                (file, false)
            }
            // The directory went away: a refused run that made it is
            // cleaning up, so another run was here.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(WriteError::Locked(path));
            }
            Err(source) => return Err(WriteError::Io { path, source }),
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(WriteError::Locked(path)),
            Err(fs::TryLockError::Error(source)) => return Err(WriteError::Io { path, source }),
        }
        still_there(&file, &path)?;
        Ok(DirLock {
            _file: file,
            made: made.then_some(path),
        })
    }

    /// Shared, on another output root this run can reach: runs under one
    /// enclosing root share it, while that root's own run, holding it
    /// exclusively, keeps them out. An existing lock file is opened for
    /// reading only, and one that cannot be opened is an error. Without
    /// `create` a directory with no lock file is no root, and one where it
    /// cannot be created is skipped: a run rooted there creates its own
    /// lock file before looking for this run's.
    fn share(dir: &Path, create: bool) -> Result<Option<DirLock>, WriteError> {
        check_lock(dir)?;
        let path = dir.join(LOCK_FILE);
        let cannot_create = |e: &std::io::Error| {
            matches!(
                e.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
            )
        };
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            // An existing lock that cannot be opened may be held: refuse
            // rather than run unsynchronised.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                if !create {
                    return Ok(None);
                }
                match fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&path)
                {
                    Ok(file) => file,
                    Err(e) if cannot_create(&e) => return Ok(None),
                    Err(source) => return Err(WriteError::Io { path, source }),
                }
            }
            Err(source) => return Err(WriteError::Io { path, source }),
        };
        match file.try_lock_shared() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(WriteError::Locked(path)),
            Err(fs::TryLockError::Error(source)) => return Err(WriteError::Io { path, source }),
        }
        still_there(&file, &path)?;
        Ok(Some(DirLock {
            _file: file,
            made: None,
        }))
    }
}

/// A lock is only worth holding on the file the path names: one removed
/// between this run's open and its lock (by a refused run that created it)
/// guards nothing, so the run stops as if the lock were held.
fn still_there(file: &fs::File, path: &Path) -> Result<(), WriteError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let held = file.metadata().map_err(io(path))?;
        match fs::metadata(path) {
            Ok(now) if now.dev() == held.dev() && now.ino() == held.ino() => Ok(()),
            _ => Err(WriteError::Locked(path.to_path_buf())),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (file, path);
        Ok(())
    }
}

/// The lock path must be a regular file or absent; [`check()`] looks too,
/// so it never passes a directory a write would refuse.
fn check_lock(out_dir: &Path) -> Result<(), WriteError> {
    let path = out_dir.join(LOCK_FILE);
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() => Err(WriteError::BadLock(path)),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(WriteError::Io { path, source }),
    }
}

fn language(target: Target) -> &'static str {
    match target {
        Target::Rust => "rust",
        Target::Go => "go",
    }
}

/// The manifest's file name for a target: `.roas-codegen-<label>-<language>.json`.
///
/// Labels are lowercase: on a case-insensitive filesystem, `Pets` and
/// `pets` would name the same manifest and one target would take over the
/// other's files.
pub fn manifest_name(label: &str, target: Target) -> Result<String, WriteError> {
    if label.is_empty()
        || !label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(WriteError::InvalidLabel(label.to_owned()));
    }
    Ok(format!(
        "{MANIFEST_PREFIX}{label}-{}{MANIFEST_SUFFIX}",
        language(target)
    ))
}

/// FNV-1a, 64 bits: stable across Rust versions and platforms, which is
/// all a content check between runs needs.
fn hash(content: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in content {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{h:016x}")
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> WriteError + '_ {
    move |source| WriteError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Whether Windows reaches a file by this name and no other: not an 8.3
/// short name (`PETSTO~1.RS`), an alternate data stream (`a.rs:x`) or a
/// device (`aux.rs`, `con`), which it resolves elsewhere whatever follows.
/// Refused on every platform, as one rule.
pub(crate) fn windows_safe(part: &str) -> bool {
    // Characters Windows forbids in names, `:` streams among them.
    if part
        .bytes()
        .any(|b| b < 0x20 || matches!(b, b':' | b'\\' | b'"' | b'*' | b'<' | b'>' | b'?' | b'|'))
    {
        return false;
    }
    let bytes = part.as_bytes();
    if bytes
        .windows(2)
        .any(|w| w[0] == b'~' && w[1].is_ascii_digit())
    {
        return false;
    }
    let stem = part
        .split('.')
        .next()
        .unwrap_or(part)
        .trim_end()
        .to_ascii_lowercase();
    let device = matches!(
        stem.as_str(),
        "con" | "prn" | "aux" | "nul" | "conin$" | "conout$"
    ) || ((stem.starts_with("com") || stem.starts_with("lpt"))
        && stem.len() == 4
        && stem.as_bytes()[3].is_ascii_digit()
        && stem.as_bytes()[3] != b'0');
    !device
}

/// A generated path as a manifest key, refusing anything that could
/// escape the output directory.
fn key(path: &Path) -> Result<String, WriteError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| WriteError::UnsafePath(path.to_path_buf()))?;
                // Case-insensitive disks fold more than ASCII case, and some
                // normalize Unicode; ownership compares ASCII case only, so
                // anything else could let two spellings share one file.
                if !part.is_ascii() {
                    return Err(WriteError::NonAsciiPath(path.to_path_buf()));
                }
                // Windows drops trailing dots and spaces, so `a.rs.` would
                // be `a.rs` there: refused everywhere, as one rule.
                if part.ends_with('.') || part.ends_with(' ') || !windows_safe(part) {
                    return Err(WriteError::UnsafePath(path.to_path_buf()));
                }
                parts.push(part.to_owned());
            }
            _ => return Err(WriteError::UnsafePath(path.to_path_buf())),
        }
    }
    if parts.is_empty() {
        return Err(WriteError::UnsafePath(path.to_path_buf()));
    }
    Ok(parts.join("/"))
}

/// Whether any component of a manifest key is a name the writer uses for
/// itself: a directory named like a manifest would be read as one, and an
/// entry naming the lock or a manifest would let a run delete it.
fn is_reserved(key: &str) -> bool {
    key.to_ascii_lowercase().split('/').any(|name| {
        (name.starts_with(MANIFEST_PREFIX) && name.ends_with(MANIFEST_SUFFIX))
            || name.contains(TMP_MARKER)
            || name == LOCK_FILE
    })
}

fn read_manifest(path: &Path) -> Result<Option<Manifest>, WriteError> {
    // Looked at before opening: a link is never followed, and a FIFO
    // named like a manifest would block the read for ever.
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            // Manifests sit in the output directory: named relative to it.
            return Err(WriteError::Symlink(
                path.file_name().map(PathBuf::from).unwrap_or_default(),
            ));
        }
        Ok(meta) if !meta.is_file() => {
            return Err(WriteError::Manifest {
                path: path.to_path_buf(),
                reason: "not a regular file".into(),
            });
        }
        _ => {}
    }
    match fs::read(path) {
        Ok(bytes) => {
            let manifest: Manifest =
                serde_json::from_slice(&bytes).map_err(|e| WriteError::Manifest {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                })?;
            // A manifest is a file in the repository, so whoever controls
            // the repository controls it: an entry that leaves the output
            // directory must never reach a delete or a write.
            for file in manifest.files.keys().chain(manifest.replacing.keys()) {
                if is_reserved(file) {
                    return Err(WriteError::Manifest {
                        path: path.to_path_buf(),
                        reason: format!("entry {file:?} names one of the generator's own files"),
                    });
                }
                if key(Path::new(file)).ok().as_deref() != Some(file.as_str()) {
                    return Err(WriteError::Manifest {
                        path: path.to_path_buf(),
                        reason: format!(
                            "entry {file:?} is not a relative path inside the output directory"
                        ),
                    });
                }
            }
            Ok(Some(manifest))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(WriteError::Io {
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

/// `out_dir` as an absolute path with links resolved, even when it does not
/// exist yet: the deepest existing prefix is resolved by the filesystem
/// (`..` included, so `link/..` is the link target's parent), the rest is
/// appended and normalized by its text, and what that leaves is resolved
/// again, since a `..` through a missing directory may lead back through
/// links. A run that creates the directory and one that finds it see the
/// same enclosing roots. `None` when nothing on the path exists, as on a
/// missing drive.
fn resolved(out_dir: &Path) -> Option<PathBuf> {
    let probe = if out_dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        out_dir
    };
    let absolute = std::path::absolute(probe).ok()?;
    let parts: Vec<Component> = absolute.components().collect();
    for split in (1..=parts.len()).rev() {
        let prefix: PathBuf = parts[..split].iter().collect();
        let Ok(mut out) = fs::canonicalize(&prefix) else {
            continue;
        };
        for part in &parts[split..] {
            out.push(part);
        }
        let mut lexical = PathBuf::new();
        for part in out.components() {
            match part {
                Component::ParentDir => {
                    lexical.pop();
                }
                Component::CurDir => {}
                other => lexical.push(other),
            }
        }
        return if lexical == out {
            Some(out)
        } else {
            resolved(&lexical)
        };
    }
    None
}

/// The directories above `root` a run looks in for other output roots,
/// up to the first shared, sticky one (such as `/tmp`) and not beyond:
/// what other users put there is not this run's business, and must not be
/// able to block it.
fn enclosing_dirs(root: &Path) -> impl Iterator<Item = &Path> {
    root.ancestors().skip(1).take_while(|dir| !is_sticky(dir))
}

fn is_sticky(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(dir).is_ok_and(|m| m.permissions().mode() & 0o1000 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        false
    }
}

/// The other output roots a run in `out_dir` can reach, each with whether
/// its lock file may be created: enclosing directories holding a manifest
/// (yes) or a lock file (no), and directories on the way to `keys` holding
/// a lock file (no). Deduplicated by identity, `out_dir` itself excluded:
/// one directory spelled two ways is locked once.
fn other_roots<'a>(
    out_dir: &Path,
    keys: impl IntoIterator<Item = &'a String>,
) -> Vec<(PathBuf, bool)> {
    let has = |dir: &Path, manifest: bool| {
        fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if manifest {
                    is_manifest_name(&name)
                } else {
                    name == LOCK_FILE
                }
            })
        })
    };
    let mut roots = Vec::new();
    if let Some(root) = resolved(out_dir) {
        for dir in enclosing_dirs(&root) {
            if has(dir, true) {
                roots.push((dir.to_path_buf(), true));
            } else if has(dir, false) {
                roots.push((dir.to_path_buf(), false));
            }
        }
    }
    // Each directory once, however many files sit below it.
    let prefixes: BTreeSet<&str> = keys
        .into_iter()
        .flat_map(|key| key.match_indices('/').map(move |(i, _)| &key[..i]))
        .collect();
    for prefix in prefixes {
        {
            for rel in case_variants(out_dir, prefix) {
                let dir = out_dir.join(rel);
                if has(&dir, false) {
                    roots.push((dir, false));
                }
            }
        }
    }
    let mut seen = Vec::new();
    if let Some(id) = file_id(out_dir) {
        seen.push(id);
    }
    roots.retain(|(dir, _)| match file_id(dir) {
        Some(id) if seen.contains(&id) => false,
        Some(id) => {
            seen.push(id);
            true
        }
        None => true,
    });
    roots
}

/// Directories a run created, removed again (if empty) unless the run
/// succeeds, so a refused run leaves no directory or lock file of its own
/// behind (a lock file it made in an enclosing root, which already holds
/// a manifest, stays, as all lock files do). Lock files other runs
/// made are never touched.
struct Created {
    dirs: Vec<PathBuf>,
    keep: bool,
    /// The run's own lock, released here: after its file is removed, if
    /// the run created that file and is refused.
    lock: Option<DirLock>,
    /// Shared locks on other roots, released last, after any cleanup: an
    /// enclosing run cannot start writing into a directory this run is
    /// about to remove.
    shared: Vec<DirLock>,
}

impl Created {
    /// Create `dir` and whatever leading to it is missing, remembering the
    /// directories this made. `..` and `.` name directories made earlier
    /// or already there, so they are created through, never remembered.
    fn dir_all(&mut self, dir: &Path) -> Result<(), WriteError> {
        let missing: Vec<PathBuf> = dir
            .ancestors()
            .filter(|a| {
                matches!(a.components().next_back(), Some(Component::Normal(_)))
                    && fs::symlink_metadata(a).is_err()
            })
            .map(Path::to_path_buf)
            .collect();
        fs::create_dir_all(dir).map_err(io(dir))?;
        self.dirs.extend(missing.into_iter().rev());
        Ok(())
    }
}

impl Drop for Created {
    fn drop(&mut self) {
        let lock = self.lock.take();
        if self.keep {
            return;
        }
        // Still locked while removed, so no run can take it in between;
        // a run that opened it before finds it gone and stops.
        // Only where `still_there` can see it: elsewhere a run that opened
        // the file first could lock a removed one, so it stays.
        #[cfg(unix)]
        if let Some(path) = lock.as_ref().and_then(|l| l.made.as_ref()) {
            let _ = fs::remove_file(path);
        }
        // Directories too, before the lock goes: a run that made the lock
        // file made the directory, and no other run can be past its lock.
        for dir in self.dirs.iter().rev() {
            let _ = fs::remove_dir(dir);
        }
        drop(lock);
    }
}

/// The directories below `out_dir` whose path is `rel` ignoring ASCII
/// case, as stored: on a case-sensitive disk `Sub` and `sub` are two, and
/// a nested root in either owns what ownership, compared ignoring case,
/// says it owns. Plus `rel` itself when the disk resolves it, for foldings
/// beyond ASCII. Links are not followed.
fn case_variants(out_dir: &Path, rel: &str) -> Vec<String> {
    let mut found = vec![String::new()];
    for part in rel.split('/') {
        let mut next = Vec::new();
        for base in &found {
            let dir = if base.is_empty() {
                out_dir.to_path_buf()
            } else {
                out_dir.join(base)
            };
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.eq_ignore_ascii_case(part) && entry.file_type().is_ok_and(|t| t.is_dir()) {
                    next.push(if base.is_empty() {
                        name
                    } else {
                        format!("{base}/{name}")
                    });
                }
            }
        }
        found = next;
    }
    // And the path as spelled, where the disk resolves it to a directory
    // stored under some other folding (Unicode case, normalization), with
    // no link on the way.
    let mut current = out_dir.to_path_buf();
    let mut is_dir = true;
    for part in rel.split('/') {
        current.push(part);
        if !fs::symlink_metadata(&current).is_ok_and(|m| m.is_dir()) {
            is_dir = false;
            break;
        }
    }
    if is_dir && !found.iter().any(|f| f == rel) {
        found.push(rel.to_owned());
    }
    found
}

/// Go files in one directory are one package, so two Go generation targets
/// with files in one directory would declare the same helpers and fail to
/// compile: a directory this generation puts a `.go` file in must hold no
/// `.go` file another Go target's manifest owns, wherever that manifest is.
fn check_go_package(
    out_dir: &Path,
    manifest_file: &str,
    planned: &BTreeMap<String, &str>,
) -> Result<(), WriteError> {
    fn dir_of(key: &str) -> String {
        key.rsplit_once('/')
            .map_or(String::new(), |(dir, _)| dir.to_ascii_lowercase())
    }
    let ours: BTreeSet<String> = planned
        .keys()
        .filter(|key| key.to_ascii_lowercase().ends_with(".go"))
        .map(|key| dir_of(key))
        .collect();
    if ours.is_empty() {
        return Ok(());
    }
    let suffix = format!("-{}{MANIFEST_SUFFIX}", language(Target::Go));
    for (path, manifest) in other_owners(out_dir, manifest_file, planned.keys())? {
        let go_manifest = manifest
            .file_name()
            .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().ends_with(&suffix));
        if go_manifest && path.ends_with(".go") && ours.contains(&dir_of(&path)) {
            return Err(WriteError::SharedGoPackage { manifest });
        }
    }
    Ok(())
}

fn is_manifest_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with(MANIFEST_PREFIX) && lower.ends_with(MANIFEST_SUFFIX)
}

/// Every other target's manifest that can own a path below `out_dir`, and
/// the paths it owns there, keyed relative to `out_dir` in lowercase.
///
/// Output roots may nest: a target rooted in a subdirectory keeps its
/// manifest there, and one rooted above keeps it above. Both are read: the
/// first in each directory on the way to one of `keys` (only those can
/// hold a manifest owning them), the second through `out_dir`'s
/// ancestors, for their entries below `out_dir`.
fn other_owners<'a>(
    out_dir: &Path,
    ours: &str,
    keys: impl IntoIterator<Item = &'a String>,
) -> Result<BTreeMap<String, PathBuf>, WriteError> {
    let mut owners = BTreeMap::new();
    // Below, the output directory included.
    let mut dirs = BTreeSet::from([String::new()]);
    // Each directory once, however many files sit below it.
    let prefixes: BTreeSet<&str> = keys
        .into_iter()
        .flat_map(|key| key.match_indices('/').map(move |(i, _)| &key[..i]))
        .collect();
    for prefix in prefixes {
        dirs.extend(case_variants(out_dir, prefix));
    }
    for rel in dirs {
        let dir = if rel.is_empty() {
            out_dir.to_path_buf()
        } else {
            out_dir.join(&rel)
        };
        // Not a directory, or a link the path checks refuse later: nothing
        // here is read, let alone followed.
        if !rel.is_empty() && !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(WriteError::Io {
                    path: dir,
                    source: e,
                });
            }
        };
        for entry in entries {
            let entry = entry.map_err(io(&dir))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            // Named like a manifest, it is read as one whatever it is, so a
            // link or a directory under that name is refused.
            if !is_manifest_name(&name) {
                continue;
            }
            // Ours only if it is the file this run reads as its manifest:
            // on a case-sensitive disk `Pets` is someone else's.
            if rel.is_empty()
                && name.eq_ignore_ascii_case(ours)
                && (name == ours
                    || matches!(
                        (file_id(&entry.path()), file_id(&out_dir.join(ours))),
                        (Some(a), Some(b)) if a == b
                    ))
            {
                continue;
            }
            let manifest = match read_manifest(&entry.path()) {
                // Named relative to the output directory, as everywhere.
                Err(WriteError::Symlink(_)) => {
                    return Err(WriteError::Symlink(PathBuf::from(&child)));
                }
                other => other?,
            };
            if let Some(manifest) = manifest {
                for file in manifest.files.keys() {
                    let key = if rel.is_empty() {
                        file.clone()
                    } else {
                        format!("{rel}/{file}")
                    };
                    owners.insert(key.to_ascii_lowercase(), PathBuf::from(&child));
                }
            }
        }
    }
    // Above: only entries that fall inside this output directory. These
    // directories are the caller's, so what cannot be read is skipped.
    if let Some(root) = resolved(out_dir) {
        for ancestor in enclosing_dirs(&root) {
            let Ok(below_path) = root.strip_prefix(ancestor) else {
                continue;
            };
            let below_path = below_path.to_path_buf();
            let below: Vec<String> = below_path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
                .collect();
            let prefix = format!("{}/", below.join("/"));
            // The same directory under another name (case beyond ASCII,
            // Unicode folding) is found by identity, not by its text.
            let root_id = file_id(&root);
            let same_dir = |rel: &str, depth: usize| {
                let ours: PathBuf = below_path.components().take(depth).collect();
                matches!(
                    (file_id(&ancestor.join(rel)), file_id(&ancestor.join(ours))),
                    (Some(a), Some(b)) if a == b
                )
            };
            let Ok(entries) = fs::read_dir(ancestor) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !is_manifest_name(&name) || !entry.file_type().is_ok_and(|t| t.is_file()) {
                    continue;
                }
                // Unreadable is the caller's business up there, but a
                // manifest that does not parse may still own files here.
                let manifest = match read_manifest(&entry.path()) {
                    Ok(Some(manifest)) => manifest,
                    Ok(None) | Err(WriteError::Io { .. }) => continue,
                    Err(error) => return Err(error),
                };
                let own = prefix.trim_end_matches('/');
                for original in manifest.files.keys() {
                    let file = original.to_ascii_lowercase();
                    let parts: Vec<&str> = original.split('/').collect();
                    if let Some(inside) = file.strip_prefix(&prefix) {
                        owners.insert(inside.to_owned(), entry.path());
                    } else if parts.len() > below.len()
                        && root_id.is_some()
                        && file_id(&ancestor.join(parts[..below.len()].join("/"))) == root_id
                    {
                        let inside = parts[below.len()..].join("/").to_ascii_lowercase();
                        owners.insert(inside, entry.path());
                    } else if file == own
                        || own.starts_with(&format!("{file}/"))
                        || (parts.len() <= below.len() && same_dir(original, parts.len()))
                    {
                        // The output directory, or one above it, is that
                        // target's file: keyed empty, as the whole tree.
                        owners.insert(String::new(), entry.path());
                    }
                }
            }
        }
    }
    Ok(owners)
}

/// The generated files keyed for the manifest, with their contents.
fn planned(generation: &Generation) -> Result<BTreeMap<String, &str>, WriteError> {
    let mut out = BTreeMap::new();
    // Compared case-insensitively throughout: the disk may be.
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut dirs: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();
    for file in &generation.files {
        let key = key(&file.path)?;
        let lower = key.to_ascii_lowercase();
        if is_reserved(&key) {
            return Err(WriteError::ReservedPath(file.path.clone()));
        }
        // One spelling per directory: two would be one directory on a
        // case-insensitive disk and two on another.
        for (i, _) in key.match_indices('/') {
            let (dir, dir_lower) = (&key[..i], &lower[..i]);
            match dirs.get(dir_lower) {
                Some((spelled, other)) if spelled != dir => {
                    return Err(WriteError::ConflictingPaths(
                        other.clone(),
                        file.path.clone(),
                    ));
                }
                Some(_) => {}
                None => {
                    dirs.insert(dir_lower.to_owned(), (dir.to_owned(), file.path.clone()));
                }
            }
        }
        if let Some(other) = seen.insert(lower, file.path.clone()) {
            return Err(WriteError::ConflictingPaths(other, file.path.clone()));
        }
        out.insert(key, file.contents.as_str());
    }
    // A file cannot also be a directory another generated file sits in.
    for path in seen.keys() {
        let mut prefix = String::new();
        for part in path
            .split('/')
            .collect::<Vec<_>>()
            .split_last()
            .map(|(_, dirs)| dirs.to_vec())
            .unwrap_or_default()
        {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if let Some(file) = seen.get(&prefix) {
                return Err(WriteError::ConflictingPaths(
                    file.clone(),
                    seen[path].clone(),
                ));
            }
        }
    }
    Ok(out)
}

/// The output directory itself must not be a symbolic link, or every
/// check below it would vouch for a directory somewhere else.
fn check_out_dir(out_dir: &Path) -> Result<(), WriteError> {
    // A trailing separator makes the lookup follow a symlink, so the
    // final entry is inspected without one.
    let out_dir: PathBuf = out_dir.components().collect();
    let out_dir = out_dir.as_path();
    match fs::symlink_metadata(out_dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(WriteError::OutputDirSymlink(out_dir.to_path_buf()))
        }
        Ok(meta) if !meta.is_dir() => {
            Err(WriteError::OutputDirNotADirectory(out_dir.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(WriteError::Io {
            path: out_dir.to_path_buf(),
            source: e,
        }),
    }
}

/// Refuse a path that would be written through a symbolic link, or whose
/// parent is something other than a directory — checked before anything
/// is written, so the failure cannot strand a half-written run.
fn check_path(out_dir: &Path, key: &str) -> Result<(), WriteError> {
    let mut current = out_dir.to_path_buf();
    let parts: Vec<&str> = key.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        current.push(part);
        let last = i + 1 == parts.len();
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(WriteError::Symlink(PathBuf::from(parts[..=i].join("/"))));
            }
            Ok(meta) if !last && !meta.is_dir() => {
                return Err(WriteError::NotADirectory(PathBuf::from(
                    parts[..=i].join("/"),
                )));
            }
            Ok(meta) if last && meta.is_dir() => {
                return Err(WriteError::IsADirectory(PathBuf::from(key)));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(WriteError::Io {
                    path: current,
                    source: e,
                });
            }
        }
    }
    Ok(())
}

/// What a run leaves owned, and which previous files it may remove: the
/// one rule both [`write()`] and [`check()`] follow, so they cannot
/// disagree. A generation with errors is missing types, which is no proof
/// that the files describing them are obsolete, so it keeps and still owns
/// every previous file; a clean one owns exactly what it generates.
fn outcome(
    previous: &BTreeMap<String, String>,
    planned: &BTreeMap<String, &str>,
    aliased: &BTreeSet<String>,
    has_errors: bool,
) -> (BTreeMap<String, String>, Vec<(String, String)>) {
    let mut owned: BTreeMap<String, String> = planned
        .iter()
        .map(|(p, c)| (p.clone(), hash(c.as_bytes())))
        .collect();
    let mut removable = Vec::new();
    for (path, recorded) in previous {
        // Planned again, or the same file under another spelling on a
        // case-insensitive disk: never stale.
        if planned.contains_key(path) || aliased.contains(path) {
            continue;
        }
        if has_errors {
            owned.insert(path.clone(), recorded.clone());
        } else {
            removable.push((path.clone(), recorded.clone()));
        }
    }
    (owned, removable)
}

/// What identifies a file on disk, whatever spelling reaches it.
#[cfg(unix)]
fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// What identifies a file on disk: the final path, which carries the
/// spelling stored on disk.
#[cfg(not(unix))]
fn file_id(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok()
}

/// Whether a file has a single directory entry. A file with several may be
/// reached through hard links the user made, which are never spellings.
fn single_link(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path).is_ok_and(|m| m.nlink() == 1)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

/// Whether two paths name one file, as differently cased or normalized
/// spellings do on a case-insensitive disk. Missing paths are distinct.
#[cfg(test)]
fn same_file(a: &Path, b: &Path) -> bool {
    matches!((file_id(a), file_id(b)), (Some(a), Some(b)) if a == b)
}

/// Previous paths that are, on this disk, the same file as a planned path
/// spelled differently, keyed to the planned path. Decided by file
/// identity alone: what the disk folds (case beyond ASCII, Unicode
/// normalization) is not for this code to guess.
fn aliases(
    out_dir: &Path,
    previous: &BTreeMap<String, String>,
    planned: &BTreeMap<String, &str>,
) -> BTreeMap<String, String> {
    let ids: BTreeMap<_, &String> = planned
        .keys()
        .filter_map(|p| file_id(&out_dir.join(p)).map(|id| (id, p)))
        .collect();
    previous
        .keys()
        .filter(|p| !planned.contains_key(*p))
        .filter_map(|old| {
            let on_disk = out_dir.join(old);
            let id = file_id(&on_disk)?;
            // Same file, same name ignoring case and one entry: a
            // respelling. A hard link is a different entry, never ours.
            if !single_link(&on_disk) {
                return None;
            }
            ids.get(&id)
                .filter(|new| new.eq_ignore_ascii_case(old))
                .map(|new| (old.clone(), (*new).clone()))
        })
        .collect()
}

/// Previous paths spelled like a planned one ignoring ASCII case that no
/// longer exist: the file now has the planned spelling, or none. A partial
/// run keeps every other previous path, but these would be dropped by the
/// next run anyway (as aliases, once the planned file exists), so they are
/// dropped now and both runs agree.
fn gone_spellings(
    out_dir: &Path,
    previous: &BTreeMap<String, String>,
    planned: &BTreeMap<String, &str>,
) -> Vec<String> {
    previous
        .keys()
        .filter(|old| !planned.contains_key(*old))
        .filter(|old| planned.keys().any(|new| new.eq_ignore_ascii_case(old)))
        .filter(|old| fs::symlink_metadata(out_dir.join(old)).is_err())
        .cloned()
        .collect()
}

/// Give an aliased file its new spelling. Writing through the new name
/// keeps the old directory entry's spelling on a case-insensitive disk,
/// and an unchanged file is not written at all, so the manifest would
/// record a name the disk does not show. Only the file name is respelled,
/// inside the directory as it is spelled on disk: directories may hold
/// other targets' files, so their case is left alone.
fn respell(out_dir: &Path, old: &str, new: &str) -> Result<(), WriteError> {
    let from = out_dir.join(old);
    let name = new.rsplit('/').next().unwrap_or(new);
    let to = from.with_file_name(name);
    // One rename, so no crash leaves the file under a third name. APFS,
    // NTFS and case-folding ext4 apply a case-only rename; a disk that
    // ignores it keeps the old spelling, which reaches the same file.
    fs::rename(&from, &to).map_err(io(&from))
}

/// The ownership refusals both [`write()`] and [`check()`] apply, so a
/// directory that checks clean or out of date is one a write could update.
fn check_ownership(
    out_dir: &Path,
    manifest_file: &str,
    previous: &Manifest,
    planned: &BTreeMap<String, &str>,
    aliased: &BTreeMap<String, String>,
    going: &BTreeSet<String>,
    options: &WriteOptions,
) -> Result<(), WriteError> {
    let others = other_owners(
        out_dir,
        manifest_file,
        planned.keys().chain(previous.files.keys()),
    )?;
    if let Some(manifest) = others.get("") {
        return Err(WriteError::OutputDirOwned {
            manifest: manifest.clone(),
        });
    }
    for (path, content) in planned {
        let lower = path.to_ascii_lowercase();
        // The same path, a file where this one needs a directory, or a
        // directory this one would replace with a file.
        let ancestors = lower
            .match_indices('/')
            .map(|(i, _)| &lower[..i])
            .collect::<Vec<_>>();
        let conflict = others.get(&lower).or_else(|| {
            ancestors.iter().find_map(|a| others.get(*a)).or_else(|| {
                others
                    .range(format!("{lower}/")..)
                    .next()
                    .filter(|(k, _)| k.starts_with(&format!("{lower}/")))
                    .map(|(_, m)| m)
            })
        });
        if let Some(manifest) = conflict {
            return Err(WriteError::OwnedByOther {
                path: PathBuf::from(path),
                manifest: manifest.clone(),
            });
        }
        match check_path(out_dir, path) {
            // This target's own stale file, removed before anything is
            // written, where the new path needs a directory.
            Err(WriteError::NotADirectory(blocker))
                if blocker
                    .to_str()
                    .is_some_and(|b| is_going(out_dir, going, b)) => {}
            other => other?,
        }
        let on_disk = out_dir.join(path);
        if !options.overwrite_unowned
            && !previous.files.contains_key(path)
            && !aliased.values().any(|new| new == path)
            // Another spelling of one of our stale files, removed first.
            && !is_going(out_dir, going, path)
            && on_disk.exists()
            // Already exactly what would be written: taking it over loses
            // nothing, which is how a deleted manifest is recovered.
            && read_recorded(&on_disk)?.as_deref() != Some(content.as_bytes())
        {
            return Err(WriteError::Unowned(PathBuf::from(path)));
        }
    }
    for path in previous.files.keys() {
        // No longer generated: only a link matters. A file or directory in
        // its way means it is gone, which is what the run is after anyway.
        if !planned.contains_key(path) {
            check_links(out_dir, path)?;
            continue;
        }
        check_recorded(out_dir, path, going)?;
    }
    // Last, so whose files these are is the first thing said.
    check_directory_spelling(out_dir, planned)?;
    check_path(out_dir, manifest_file)
}

/// The stale files a run removes: those still holding what this target
/// wrote. The rest were edited and are kept.
fn going(
    out_dir: &Path,
    manifest_file: &str,
    previous: &Manifest,
    removable: &[(String, String)],
) -> Result<BTreeSet<String>, WriteError> {
    // A file another manifest also lists, as after merging two branches
    // that each added a target, is not this target's alone to delete.
    let others = other_owners(out_dir, manifest_file, removable.iter().map(|(p, _)| p))?;
    let mut out = BTreeSet::new();
    for (path, _) in removable {
        if others.contains_key(&path.to_ascii_lowercase()) {
            continue;
        }
        if read_recorded(&out_dir.join(path))?.is_some_and(|bytes| previous.wrote(path, &bytes)) {
            out.insert(path.clone());
        }
    }
    // Another recorded spelling of a file that is ours is that same file,
    // whatever its own entry says, as after an interrupted respelling.
    // Only a spelling: same file, same name ignoring case, one entry. A
    // hard link the user made under another name keeps its own verdict.
    let ours: Vec<(String, _)> = out
        .iter()
        .filter_map(|p| file_id(&out_dir.join(p)).map(|id| (p.clone(), id)))
        .collect();
    for (path, _) in removable {
        let on_disk = out_dir.join(path);
        if single_link(&on_disk)
            && file_id(&on_disk).is_some_and(|id| {
                ours.iter()
                    .any(|(p, other)| *other == id && p.eq_ignore_ascii_case(path))
            })
        {
            out.insert(path.clone());
        }
    }
    // One file, one verdict: a spelling that is not ours keeps every
    // spelling of that file, as check reports it once.
    let kept: Vec<(String, _)> = removable
        .iter()
        .filter(|(p, _)| !out.contains(p))
        .filter_map(|(p, _)| file_id(&out_dir.join(p)).map(|id| (p.clone(), id)))
        .collect();
    out.retain(|p| {
        let id = file_id(&out_dir.join(p));
        !kept
            .iter()
            .any(|(k, kid)| k.eq_ignore_ascii_case(p) && Some(kid) == id.as_ref())
    });
    Ok(out)
}

/// A file's content, or `None` when there is no regular file there to be
/// ours: missing, under a path that is no directory, or anything but a
/// regular file — a directory (some platforms refuse to open one with a
/// permission error), a link, or a FIFO a read would block on. Checked
/// before opening, so nothing is followed or waited on.
fn read_recorded(path: &Path) -> Result<Option<Vec<u8>>, WriteError> {
    let missing = |e: &std::io::Error| {
        matches!(
            e.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
        )
    };
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => return Ok(None),
        Ok(_) => {}
        Err(e) if missing(&e) => return Ok(None),
        Err(source) => {
            return Err(WriteError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if missing(&e) => Ok(None),
        Err(source) => Err(WriteError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// [`check_path`] for a path the previous manifest records. One that is
/// now a directory, as after an interrupted run turned a file into the
/// directory a new path needs, is no longer this target's file: it is
/// dropped from the manifest and never deleted.
///
/// A file in the way of a recorded path is fine when it is one of this
/// target's stale files a run removes first, as after a run interrupted
/// between its pending manifest and turning that file into a directory.
fn check_recorded(out_dir: &Path, key: &str, going: &BTreeSet<String>) -> Result<(), WriteError> {
    match check_path(out_dir, key) {
        Err(WriteError::IsADirectory(_)) => Ok(()),
        Err(WriteError::NotADirectory(blocker))
            if blocker
                .to_str()
                .is_some_and(|b| is_going(out_dir, going, b)) =>
        {
            Ok(())
        }
        other => other,
    }
}

/// Whether `blocker` is one of the stale files a run removes first: by
/// name, or as another spelling of the same file on a case-insensitive disk.
fn is_going(out_dir: &Path, going: &BTreeSet<String>, blocker: &str) -> bool {
    going.contains(blocker)
        || going.iter().any(|g| {
            g.eq_ignore_ascii_case(blocker)
                && matches!(
                    (file_id(&out_dir.join(g)), file_id(&out_dir.join(blocker))),
                    (Some(a), Some(b)) if a == b
                )
        })
}

/// The first look at recorded paths, before any is read: no symbolic
/// link on the way. What is not a directory is judged later, by
/// [`check_recorded`], once the run knows which stale files go.
fn check_links(out_dir: &Path, key: &str) -> Result<(), WriteError> {
    match check_path(out_dir, key) {
        Err(WriteError::NotADirectory(_) | WriteError::IsADirectory(_)) => Ok(()),
        other => other,
    }
}

/// The output directory as the filesystem calls should see it: without
/// `.` components (`create_dir_all("gen/.")` fails while `gen` is missing),
/// and `""` as `.`, which `read_dir` would not accept.
fn current_if_empty(out_dir: &Path) -> PathBuf {
    let normalized: PathBuf = out_dir.components().collect();
    if normalized.as_os_str().is_empty() {
        return PathBuf::from(".");
    }
    // A `..` would need every directory before it to exist (`scratch` in
    // `scratch/../gen`), and one made for that would be gone in a fresh
    // checkout. So the part before the last name is resolved as
    // `resolved` does, existing directories by the filesystem and the
    // rest by text; the last name is kept, so a link there is still seen.
    if !normalized
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return normalized;
    }
    let Some(name) = normalized.file_name() else {
        return resolved(&normalized).unwrap_or(normalized);
    };
    let parent = normalized.parent().unwrap_or(Path::new("."));
    match resolved(parent) {
        Some(parent) => parent.join(name),
        None => normalized,
    }
}

fn error_count(generation: &Generation) -> usize {
    generation
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .count()
}

/// The source a manifest records: the caller's, or the one already there
/// when the caller gave none, so a run without an identity does not erase it.
fn source_of(previous: &Manifest, options: &WriteOptions) -> String {
    // An empty id is no id: it keeps the recorded one, as `None` does.
    options
        .source_id
        .clone()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| previous.source.clone())
}

fn manifest_for(
    label: &str,
    target: Target,
    source: &str,
    files: BTreeMap<String, String>,
) -> Manifest {
    Manifest {
        generator: "roas-codegen".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        label: label.to_owned(),
        language: language(target).into(),
        source: source.to_owned(),
        files,
        replacing: BTreeMap::new(),
    }
}

/// The `replacing` entries a finished run keeps: those of files it still
/// owns without having written them, which may hold either content.
fn still_replacing(
    previous: &Manifest,
    owned: &BTreeMap<String, String>,
    planned: &BTreeMap<String, &str>,
) -> BTreeMap<String, String> {
    previous
        .replacing
        .iter()
        .filter(|(p, _)| owned.contains_key(*p) && !planned.contains_key(*p))
        .map(|(p, h)| (p.clone(), h.clone()))
        .collect()
}

fn manifest_text(manifest: &Manifest) -> String {
    let mut text = serde_json::to_string_pretty(manifest).expect("a manifest serializes");
    text.push('\n');
    text
}

/// Refuse a run whose description differs from the one that owns the
/// label: two descriptions with the same file name in different folders
/// would otherwise overwrite each other's output without a word.
fn check_source(previous: &Manifest, options: &WriteOptions) -> Result<(), WriteError> {
    let current = options.source_id.as_deref().unwrap_or_default();
    if !options.replace_source
        && !previous.source.is_empty()
        && !current.is_empty()
        && previous.source != current
    {
        return Err(WriteError::SourceMismatch {
            label: options.label.clone(),
            previous: previous.source.clone(),
            current: current.to_owned(),
        });
    }
    Ok(())
}

/// The stale paths another manifest also lists, as after merging two
/// branches that each added a target: kept, and not reported as edited.
fn shared(
    out_dir: &Path,
    manifest_file: &str,
    removable: &[(String, String)],
) -> Result<BTreeSet<String>, WriteError> {
    let others = other_owners(out_dir, manifest_file, removable.iter().map(|(p, _)| p))?;
    Ok(removable
        .iter()
        .filter(|(p, _)| others.contains_key(&p.to_ascii_lowercase()))
        .map(|(p, _)| p.clone())
        .collect())
}

/// The entry a planned path reaches under another spelling of its file
/// name, as on a case-insensitive disk where a file was taken over or
/// renamed by hand: the manifest would record a name the disk does not
/// show. `None` when the exact name is there, or the variant is not the
/// same single-link file.
fn misspelled(out_dir: &Path, key: &str, listings: &mut Listings) -> Option<String> {
    misspelled_entry(out_dir, key, listings).and_then(|(stored, single)| single.then_some(stored))
}

/// [`misspelled`]'s entry, whatever its link count, with whether it is the
/// file's only name. A hard-linked one is not renamed, but rewritten: that
/// gives the planned name a file of its own, which can then be respelled.
fn misspelled_entry(out_dir: &Path, key: &str, listings: &mut Listings) -> Option<(String, bool)> {
    let path = out_dir.join(key);
    let name = key.rsplit('/').next()?;
    // Stored as planned, as nearly every file is: one listing per directory
    // answers that without a stat per entry.
    if listings.has(path.parent()?, name) {
        return None;
    }
    // The planned name reaches a file; which directory entry is it? Found
    // by identity, not by spelling: the disk may fold beyond ASCII case.
    let wanted = file_id(&path)?;
    let entries = fs::read_dir(path.parent()?).ok()?;
    let mut variant = None;
    for entry in entries.flatten() {
        let stored = entry.file_name().to_string_lossy().into_owned();
        if stored == name {
            return None;
        }
        // A regular file only: a symlink to it is another entry, which a
        // rename must never replace.
        if entry.file_type().is_ok_and(|t| t.is_file())
            && file_id(&entry.path()).is_some_and(|id| id == wanted)
        {
            variant = Some(stored);
        }
    }
    let stored = variant?;
    let single = single_link(&path.with_file_name(&stored));
    let stored = match key.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/{stored}"),
        None => stored,
    };
    Some((stored, single))
}

/// Whether a previous path is, on this disk, the file a planned path names
/// in another case: it is being written, not kept, whatever it held.
fn is_planned_file(out_dir: &Path, planned: &BTreeMap<String, &str>, path: &str) -> bool {
    let id = file_id(&out_dir.join(path));
    id.is_some()
        && planned
            .keys()
            .any(|p| p.eq_ignore_ascii_case(path) && file_id(&out_dir.join(p)) == id)
}

/// Refuse a planned directory the disk stores under another spelling of
/// the same directory (`Dir` for `dir`): files are respelled, directories
/// are not, since others' files may live in them, and a manifest naming
/// `dir/a.rs` over `Dir/a.rs` breaks a case-sensitive checkout.
fn check_directory_spelling(
    out_dir: &Path,
    planned: &BTreeMap<String, &str>,
) -> Result<(), WriteError> {
    let prefixes: BTreeSet<&str> = planned
        .keys()
        .flat_map(|key| key.match_indices('/').map(move |(i, _)| &key[..i]))
        .collect();
    let mut listings = Listings::default();
    for prefix in prefixes {
        let path = out_dir.join(prefix);
        let Some(wanted) = file_id(&path) else {
            continue;
        };
        let (parent, name) = match prefix.rsplit_once('/') {
            Some((dir, name)) => (out_dir.join(dir), name),
            None => (out_dir.to_path_buf(), prefix),
        };
        if listings.has(&parent, name) {
            continue;
        }
        let stored = fs::read_dir(&parent).ok().and_then(|entries| {
            entries.flatten().find_map(|e| {
                (e.file_type().is_ok_and(|t| t.is_dir())
                    && file_id(&e.path()).is_some_and(|id| id == wanted))
                .then(|| e.file_name())
            })
        });
        if let Some(stored) = stored {
            let stored = match prefix.rsplit_once('/') {
                Some((dir, _)) => Path::new(dir).join(stored),
                None => PathBuf::from(stored),
            };
            return Err(WriteError::DirectorySpelling {
                planned: PathBuf::from(prefix),
                stored,
            });
        }
    }
    Ok(())
}

/// Directory listings, read once per run.
#[derive(Default)]
struct Listings(BTreeMap<PathBuf, BTreeSet<std::ffi::OsString>>);

impl Listings {
    fn has(&mut self, dir: &Path, name: &str) -> bool {
        self.0
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                fs::read_dir(dir)
                    .map(|entries| entries.flatten().map(|e| e.file_name()).collect())
                    .unwrap_or_default()
            })
            .contains(std::ffi::OsStr::new(name))
    }
}

/// Remove the temporary files a killed run left beside the files it was
/// writing. They hold generated content only, and with the lock held no
/// other run can be writing them. Only directories this run writes into
/// are looked at, built from planned paths that already passed every
/// check, and only regular files named the way [`write_atomic`] names the
/// temporaries of this run's own files.
fn remove_leftover_temps<'a>(
    out_dir: &Path,
    keys: impl IntoIterator<Item = &'a String>,
    manifest_file: &str,
) -> Result<(), WriteError> {
    // Only temporaries of this label's files, planned or recorded (one no
    // longer generated may have been cut off mid-write): a run rooted in a
    // subdirectory holds its own lock, and its temporaries are its own.
    let mut prefixes: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    prefixes
        .entry(out_dir.to_path_buf())
        .or_default()
        .insert(tmp_prefix(manifest_file));
    for key in keys {
        let (dir, name) = match key.rsplit_once('/') {
            Some((parent, name)) => (out_dir.join(parent), name),
            None => (out_dir.to_path_buf(), key.as_str()),
        };
        prefixes.entry(dir).or_default().insert(tmp_prefix(name));
    }
    for (dir, prefixes) in prefixes {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let entry = entry.map_err(io(&dir))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if prefixes.iter().any(|p| name.starts_with(p.as_str()))
                && entry.file_type().is_ok_and(|t| t.is_file())
            {
                let path = entry.path();
                fs::remove_file(&path).map_err(io(&path))?;
            }
        }
    }
    Ok(())
}

/// The start of every temporary name for a file called `name`: a short
/// hash, not the name itself, so a name near the filesystem's limit still
/// has room for the suffix, and cleanup can tell whose temporary it is.
fn tmp_prefix(name: &str) -> String {
    let digest = hash(name.as_bytes());
    let digest = digest.trim_start_matches("fnv1a64:");
    format!(".{digest}{TMP_MARKER}")
}

/// Write `content` to `path` through a temporary sibling and a rename.
fn write_atomic(path: &Path, content: &[u8]) -> Result<(), WriteError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // The temporary file is created exclusively under a name nobody can
    // predict: `create_new` refuses an existing path, symlink included, so
    // a planted link can never redirect the write.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut recreated = false;
    let (tmp, mut file) = loop {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_file_name(format!(
            "{}{}-{nanos:x}-{n}",
            tmp_prefix(&name),
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // The directory went away after it was made: a refused run that
            // created it found it empty and removed it. Made again, once.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !recreated => {
                recreated = true;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(io(parent))?;
                }
            }
            Err(e) => {
                return Err(WriteError::Io {
                    path: tmp,
                    source: e,
                });
            }
        }
    };
    let result = (|| {
        file.write_all(content).map_err(io(&tmp))?;
        file.sync_all().map_err(io(&tmp))?;
        // The file it replaces keeps its permissions, read-only included.
        if let Ok(old) = fs::symlink_metadata(path)
            && old.is_file()
        {
            fs::set_permissions(&tmp, old.permissions()).map_err(io(&tmp))?;
        }
        fs::rename(&tmp, path).map_err(io(path))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Write a generation into `out_dir`.
///
/// `generate` writes no output files; this does, under these rules.
///
/// - **Ownership is recorded.** A manifest in the output directory lists
///   every file a generation target wrote, with a content hash. Its name
///   is derived from the target — a caller-chosen label plus the language
///   — so the next run finds it, and two targets can share a directory:
///   `petstore` Rust and `billing` Rust each keep their own manifest.
/// - **Nothing is taken that is not owned.** A path another target's
///   manifest owns is refused, compared case-insensitively since the disk
///   may be. So is an existing file no manifest owns, unless the caller
///   allows overwriting it or it already holds exactly the generated
///   content, in which case it is adopted under the planned spelling.
/// - **One run at a time.** An operating-system lock on the output
///   directory's `.roas-codegen.lock` spans the whole run, exclusive, so
///   two targets sharing a directory cannot both pass the ownership checks
///   and then race. Other output roots the run can reach — enclosing ones,
///   and nested ones it writes into — are locked shared: siblings under
///   one root run side by side, while overlapping roots exclude each
///   other. Locks end with the process, so a killed run does not block the
///   next; lock files stay, as they mark the roots.
/// - **Nothing below the output directory is followed.** A symbolic link
///   as the output directory or on any generated path is refused. The
///   path *to* the output directory is the caller's choice and is used as
///   given: refusing links there would reject ordinary layouts, such as
///   macOS's `/tmp`.
/// - **Ownership is claimed before anything is written.** Every check runs
///   first. Then a pending manifest is written that owns both the previous
///   run's files and this run's, each file is written to a temporary
///   sibling and renamed into place, and the final manifest replaces the
///   pending one. A run that stops midway leaves every file it touched
///   owned, so the next run simply rewrites whatever differs. A directory
///   holding hand-written files cannot be replaced atomically as a set, so
///   the guarantee is per file, not per batch.
///
/// Only files recorded in the previous manifest are removed as stale, and
/// only when they still hold what was generated: an edited file is kept
/// and reported.
///
/// Refuses a generation that carries any `Error` diagnostic unless
/// [`WriteOptions::allow_partial`] is set: writing it would replace good
/// files with a set missing the types the errors suppressed.
pub fn write(
    generation: &Generation,
    out_dir: &Path,
    options: &WriteOptions,
) -> Result<WriteReport, WriteError> {
    let out_dir = current_if_empty(out_dir);
    let out_dir = out_dir.as_path();
    let errors = error_count(generation);
    if errors > 0 && !options.allow_partial {
        return Err(WriteError::HasErrors(errors));
    }
    let target = generation.target;
    let manifest_file = manifest_name(&options.label, target)?;
    // Refusals that need no directory come before one is created: a
    // missing directory holds no manifest or file to refuse anything else.
    let planned = planned(generation)?;
    check_out_dir(out_dir)?;
    let mut created = Created {
        dirs: Vec::new(),
        keep: false,
        lock: None,
        shared: Vec::new(),
    };
    // One run at a time over every file this one can touch, and a lock
    // held elsewhere fails at once: runs never wait. Enclosing roots come
    // first, before anything is created, so a run refused there has made
    // nothing to remove. (A run refused later, by a nested root's lock,
    // can still remove an empty directory it made while an enclosing run
    // that started after it is about to write there; that run makes the
    // directory again.) Then this run's own directory, exclusively; only after
    // that are all other roots looked for again — enclosing ones too, so one
    // whose lock file appeared meanwhile is not missed — and nested roots'
    // lock files: of two overlapping runs, each takes its own lock before
    // looking for the other's, so at least one sees it.
    let mut enclosing = Vec::new();
    for (dir, create) in other_roots(out_dir, std::iter::empty()) {
        // Only one actually locked is done: one whose lock file vanished
        // or could not be made is looked at again below.
        if let Some(lock) = DirLock::share(&dir, create)? {
            created.shared.push(lock);
            enclosing.push(dir);
        }
    }
    created.dir_all(out_dir)?;
    check_out_dir(out_dir)?;
    created.lock = Some(DirLock::acquire(out_dir)?);
    for (dir, create) in other_roots(out_dir, planned.keys()) {
        if !enclosing.contains(&dir) {
            created.shared.extend(DirLock::share(&dir, create)?);
        }
    }
    check_go_package(out_dir, &manifest_file, &planned)?;
    let manifest_path = out_dir.join(&manifest_file);
    let previous = read_manifest(&manifest_path)?.unwrap_or_default();
    check_source(&previous, options)?;
    for path in previous.files.keys() {
        check_links(out_dir, path)?;
    }
    let aliased = aliases(out_dir, &previous.files, &planned);
    let mut old_spellings: BTreeSet<String> = aliased.keys().cloned().collect();
    old_spellings.extend(gone_spellings(out_dir, &previous.files, &planned));
    let (mut owned, removable) = outcome(&previous.files, &planned, &old_spellings, errors > 0);
    let going = going(out_dir, &manifest_file, &previous, &removable)?;
    // Every check before the first write.
    check_ownership(
        out_dir,
        &manifest_file,
        &previous,
        &planned,
        &aliased,
        &going,
        options,
    )?;
    // Only now: every planned path is known to stay inside the output
    // directory and to reach it through no link.
    remove_leftover_temps(
        out_dir,
        planned.keys().chain(previous.files.keys()),
        &manifest_file,
    )?;

    let hashes: BTreeMap<String, String> = planned
        .iter()
        .map(|(p, c)| (p.clone(), hash(c.as_bytes())))
        .collect();

    // Claim ownership of everything this run may touch before touching it,
    // keeping what each file held before in case the run is interrupted.
    let mut replacing = previous.replacing.clone();
    // A file reached through an old spelling is replaced like any other:
    // until it is rewritten, it holds what the old spelling recorded.
    for (old, new) in &aliased {
        let current = match read_recorded(&out_dir.join(old))? {
            Some(bytes) if previous.wrote(old, &bytes) => hash(&bytes),
            _ => previous.files[old].clone(),
        };
        if current != hashes[new] {
            replacing.insert(new.clone(), current);
        }
    }
    for (path, new) in &hashes {
        let Some(recorded) = previous.files.get(path) else {
            continue;
        };
        // What the file holds now, when it is ours: after an interrupted
        // run that may be either hash the previous manifest records.
        let current = match read_recorded(&out_dir.join(path))? {
            Some(bytes) if previous.wrote(path, &bytes) => hash(&bytes),
            _ => recorded.clone(),
        };
        if &current == new {
            replacing.remove(path);
        } else {
            replacing.insert(path.clone(), current);
        }
    }
    let mut pending = previous.files.clone();
    pending.extend(hashes.clone());
    let mut pending = manifest_for(
        &options.label,
        target,
        &source_of(&previous, options),
        pending,
    );
    pending.replacing = replacing;
    fs::create_dir_all(out_dir).map_err(io(out_dir))?;
    if !pending.same_as(&previous) {
        write_atomic(&manifest_path, manifest_text(&pending).as_bytes())?;
    }
    // Ownership is on disk from here: a run that fails now is unfinished,
    // not refused, so its directories and lock file stay for the next.
    created.keep = true;

    let mut report = WriteReport {
        manifest: PathBuf::from(&manifest_file),
        ..Default::default()
    };
    for (old, new) in &aliased {
        let name = |p: &str| p.rsplit('/').next().map(str::to_owned);
        // Only a directory's case differs: nothing to rename.
        if name(old) == name(new) {
            continue;
        }
        respell(out_dir, old, new)?;
        report
            .renamed
            .push((PathBuf::from(old), PathBuf::from(new)));
    }
    let shared = shared(out_dir, &manifest_file, &removable)?;
    // Stale files go first: one may sit where a new path needs a directory.
    for (path, _) in &removable {
        let on_disk = out_dir.join(path);
        if going.contains(path) {
            match fs::remove_file(&on_disk) {
                Ok(()) => report.removed.push(PathBuf::from(path)),
                // Already removed through another spelling.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(WriteError::Io {
                        path: on_disk,
                        source,
                    });
                }
            }
        } else if !shared.contains(path)
            && !is_planned_file(out_dir, &planned, path)
            && read_recorded(&on_disk)?.is_some()
        {
            // Once per file, whatever spellings recorded it, as check reports.
            let id = file_id(&on_disk);
            if !report.kept_edited.iter().any(|p| {
                p.to_string_lossy().eq_ignore_ascii_case(path) && file_id(&out_dir.join(p)) == id
            }) {
                report.kept_edited.push(PathBuf::from(path));
            }
        }
    }
    let mut listings = Listings::default();
    for path in planned.keys() {
        if let Some(stored) = misspelled(out_dir, path, &mut listings) {
            respell(out_dir, &stored, path)?;
            report
                .renamed
                .push((PathBuf::from(stored), PathBuf::from(path)));
        }
    }
    let mut linked = Listings::default();
    for (path, content) in &planned {
        let on_disk = out_dir.join(path);
        // Identical, but stored under another spelling held by a hard link:
        // rewritten, so it can be respelled below.
        let misspelled_link = misspelled_entry(out_dir, path, &mut linked).is_some();
        if !misspelled_link && read_recorded(&on_disk)?.as_deref() == Some(content.as_bytes()) {
            report.unchanged.push(PathBuf::from(path));
        } else {
            write_atomic(&on_disk, content.as_bytes())?;
            report.written.push(PathBuf::from(path));
        }
    }
    // A write through a name the disk folds lands in the entry stored under
    // the old spelling. One that kept its old spelling only because a hard
    // link held it is a single file now: it gets the planned spelling, and
    // any old record of it goes, in this run rather than the next.
    let mut after = Listings::default();
    for path in report.written.clone() {
        let key = path.to_string_lossy().into_owned();
        if let Some(stored) = misspelled(out_dir, &key, &mut after) {
            respell(out_dir, &stored, &key)?;
            report.renamed.push((PathBuf::from(stored), path));
        }
    }
    for old in aliases(out_dir, &previous.files, &planned).keys() {
        owned.remove(old);
    }

    let replacing = still_replacing(&previous, &owned, &planned);
    let mut final_manifest = manifest_for(
        &options.label,
        target,
        &source_of(&previous, options),
        owned,
    );
    final_manifest.replacing = replacing;
    let on_disk = read_manifest(&manifest_path)?;
    // Rewritten also when stored under another spelling held by a hard
    // link: that gives it a file of its own to respell below.
    let linked = misspelled_entry(out_dir, &manifest_file, &mut Listings::default())
        .is_some_and(|(_, single)| !single);
    if linked || !on_disk.is_some_and(|m| m.same_as(&final_manifest)) {
        write_atomic(&manifest_path, manifest_text(&final_manifest).as_bytes())?;
    }
    // The manifest's own name as stored, too: under another spelling it is
    // another target's manifest on a case-sensitive checkout.
    if let Some(stored) = misspelled(out_dir, &manifest_file, &mut Listings::default()) {
        respell(out_dir, &stored, &manifest_file)?;
    }
    created.keep = true;
    Ok(report)
}

/// Compare `out_dir` with what [`write()`] would leave there, without
/// writing anything: file contents, owned paths and the manifest itself.
/// It applies the same refusals as [`write()`] — error diagnostics without
/// `allow_partial`, symbolic links, non-directories, paths another target
/// owns, unowned files in the way — so a directory reported out of date is
/// one a write could update.
pub fn check(
    generation: &Generation,
    out_dir: &Path,
    options: &WriteOptions,
) -> Result<CheckReport, WriteError> {
    let out_dir = current_if_empty(out_dir);
    let out_dir = out_dir.as_path();
    let errors = error_count(generation);
    if errors > 0 && !options.allow_partial {
        return Err(WriteError::HasErrors(errors));
    }
    let target = generation.target;
    let manifest_file = manifest_name(&options.label, target)?;
    // In `write`'s order, so both give the same error for the same input.
    let planned = planned(generation)?;
    check_out_dir(out_dir)?;
    // Shared locks, held for the whole comparison: a running write holds
    // the output directory's exclusively, so `check` never reads one half
    // done. Like a write, it fails at once rather than wait, and refuses a
    // lock file it cannot open.
    let mut _locks: Vec<DirLock> = Vec::new();
    // Where there was no lock file to hold (a fresh checkout ignores them),
    // one appearing by the end may mean a write ran meanwhile.
    let mut unlocked: Vec<PathBuf> = Vec::new();
    // Taken in `write`'s order, enclosing roots first, and all before the
    // manifest is read: the lock paths a write would take, so a directory
    // a write refuses never checks as merely out of date.
    let mut enclosing = Vec::new();
    for (dir, _) in other_roots(out_dir, std::iter::empty()) {
        match DirLock::share(&dir, false)? {
            Some(lock) => _locks.push(lock),
            None => unlocked.push(dir.clone()),
        }
        enclosing.push(dir);
    }
    match DirLock::share(out_dir, false)? {
        Some(lock) => _locks.push(lock),
        None => unlocked.push(out_dir.to_path_buf()),
    }
    for (dir, _) in other_roots(out_dir, planned.keys()) {
        if enclosing.contains(&dir) {
            continue;
        }
        match DirLock::share(&dir, false)? {
            Some(lock) => _locks.push(lock),
            None => unlocked.push(dir),
        }
    }
    check_go_package(out_dir, &manifest_file, &planned)?;
    let manifest_path = out_dir.join(&manifest_file);
    // `None` until the manifest was read and validated: a check that fails
    // before that has no snapshot to compare, and so no change to see.
    let mut manifest_at_start: Option<Option<Vec<u8>>> = None;
    // Everything compared under the locks. Whatever it concludes, success
    // or an error, a write that ran meanwhile where there was no lock to
    // hold makes it moot: that is reported instead.
    let result = (|| -> Result<CheckReport, WriteError> {
        let previous = read_manifest(&manifest_path)?.unwrap_or_default();
        // As read now, to see whether a write rooted here ran meanwhile; after
        // `read_manifest`, which refuses links and non-files, and never through
        // either itself.
        manifest_at_start = Some(read_recorded(&manifest_path).ok().flatten());
        check_source(&previous, options)?;
        for path in previous.files.keys() {
            check_links(out_dir, path)?;
        }
        let mut aliased: BTreeSet<String> = aliases(out_dir, &previous.files, &planned)
            .into_keys()
            .collect();
        aliased.extend(gone_spellings(out_dir, &previous.files, &planned));
        let (owned, removable) = outcome(&previous.files, &planned, &aliased, errors > 0);
        let going = going(out_dir, &manifest_file, &previous, &removable)?;
        check_ownership(
            out_dir,
            &manifest_file,
            &previous,
            &planned,
            &aliases(out_dir, &previous.files, &planned),
            &going,
            options,
        )?;
        let shared = shared(out_dir, &manifest_file, &removable)?;
        let mut report = CheckReport::default();
        let mut listings = Listings::default();
        for (path, content) in &planned {
            // Reached through a stale spelling a write removes first: written
            // afresh.
            if is_going(out_dir, &going, path) {
                report.missing.push(PathBuf::from(path));
                continue;
            }
            // Stored under another spelling: a write renames it.
            if misspelled_entry(out_dir, path, &mut listings).is_some() {
                report.changed.push(PathBuf::from(path));
                continue;
            }
            match read_recorded(&out_dir.join(path))? {
                Some(current) if current == content.as_bytes() => {}
                Some(_) => report.changed.push(PathBuf::from(path)),
                // Absent, or a stale file of ours where its directory will be.
                None => report.missing.push(PathBuf::from(path)),
            }
        }
        // One file reached through two recorded spellings is reported once,
        // as a write removes it once.
        let mut seen: Vec<(String, _)> = Vec::new();
        for (path, _) in removable {
            let on_disk = out_dir.join(&path);
            if let Some(id) = file_id(&on_disk) {
                if seen
                    .iter()
                    .any(|(p, other)| *other == id && p.eq_ignore_ascii_case(&path))
                {
                    continue;
                }
                seen.push((path.clone(), id));
            }
            if going.contains(&path) {
                report.stale.push(PathBuf::from(path));
            } else if !shared.contains(&path)
                && !is_planned_file(out_dir, &planned, &path)
                && read_recorded(&on_disk)?.is_some()
            {
                report.stale_edited.push(PathBuf::from(path));
            }
        }
        let replacing = still_replacing(&previous, &owned, &planned);
        let mut expected = manifest_for(
            &options.label,
            target,
            &source_of(&previous, options),
            owned,
        );
        expected.replacing = replacing;
        report.manifest_outdated = !read_manifest(&manifest_path)?
            .is_some_and(|m| m.same_as(&expected))
            || misspelled_entry(out_dir, &manifest_file, &mut Listings::default()).is_some();
        Ok(report)
    })();
    let manifest_changed = manifest_at_start
        .is_some_and(|start| read_recorded(&manifest_path).ok().flatten() != start);
    if let Some(lock) = appeared(out_dir, &unlocked, manifest_changed) {
        return Err(WriteError::Locked(lock));
    }
    result
}

/// A write that ran during a `check` which had no lock file to hold there:
/// a lock file that appeared and is still held exclusively, or, in the
/// output directory, one that appeared while its manifest changed (a write
/// here that already finished). A lock file a nested or sibling run made
/// only for its own shared lock touches nothing this check compared.
fn appeared(out_dir: &Path, unlocked: &[PathBuf], manifest_changed: bool) -> Option<PathBuf> {
    unlocked.iter().find_map(|dir| {
        let lock = dir.join(LOCK_FILE);
        fs::symlink_metadata(&lock).ok()?;
        let held = matches!(DirLock::share(dir, false), Err(WriteError::Locked(_)))
            || (dir == out_dir && manifest_changed);
        held.then_some(lock)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::GeneratedFile;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "roas-codegen-write-{tag}-{}-{n}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn generation(files: &[(&str, &str)]) -> Generation {
        Generation::new(
            Target::Rust,
            files
                .iter()
                .map(|(p, c)| GeneratedFile::new(*p, *c))
                .collect(),
        )
    }

    #[test]
    fn writes_records_and_then_reports_unchanged() {
        let dir = TempDir::new("basic");
        let g = generation(&[("types.rs", "a"), ("nested/more.rs", "b")]);
        let report = write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.written.len(), 2);
        assert_eq!(
            report.manifest,
            PathBuf::from(".roas-codegen-api-rust.json")
        );
        assert_eq!(
            fs::read_to_string(dir.0.join("nested/more.rs")).unwrap(),
            "b"
        );
        assert!(
            check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );

        let again = write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        assert!(again.written.is_empty());
        assert_eq!(again.unchanged.len(), 2);
        // No temporary files are left behind.
        let leftovers: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("roas-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn stale_files_are_removed_unless_edited() {
        let dir = TempDir::new("stale");
        write(
            &generation(&[("a.rs", "a"), ("b.rs", "b"), ("c.rs", "c")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        fs::write(dir.0.join("c.rs"), "edited").unwrap();
        let next = generation(&[("a.rs", "a2")]);
        assert_eq!(
            check(&next, &dir.0, &WriteOptions::new("api")).unwrap(),
            CheckReport {
                changed: vec!["a.rs".into()],
                missing: vec![],
                stale: vec!["b.rs".into()],
                stale_edited: vec!["c.rs".into()],
                manifest_outdated: true
            }
        );
        let report = write(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.written, vec![PathBuf::from("a.rs")]);
        assert_eq!(report.removed, vec![PathBuf::from("b.rs")]);
        assert_eq!(report.kept_edited, vec![PathBuf::from("c.rs")]);
        assert!(!dir.0.join("b.rs").exists());
        assert_eq!(fs::read_to_string(dir.0.join("c.rs")).unwrap(), "edited");
        // The edited file is no longer owned.
        assert!(
            check(&next, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn unowned_and_other_targets_files_are_refused_before_anything_changes() {
        let dir = TempDir::new("owners");
        fs::write(dir.0.join("types.rs"), "hand written").unwrap();
        let g = generation(&[("lib.rs", "x"), ("types.rs", "generated")]);
        let error = write(&g, &dir.0, &WriteOptions::new("api")).unwrap_err();
        assert!(
            matches!(&error, WriteError::Unowned(p) if p == Path::new("types.rs")),
            "{error}"
        );
        assert!(
            !dir.0.join("lib.rs").exists(),
            "nothing is written when a check fails"
        );
        assert_eq!(
            fs::read_to_string(dir.0.join("types.rs")).unwrap(),
            "hand written"
        );

        let mut options = WriteOptions::new("api");
        options.overwrite_unowned = true;
        write(&g, &dir.0, &options).unwrap();
        assert_eq!(
            fs::read_to_string(dir.0.join("types.rs")).unwrap(),
            "generated"
        );

        // Another label may share the directory but not the paths.
        write(
            &generation(&[("billing.rs", "b")]),
            &dir.0,
            &WriteOptions::new("billing"),
        )
        .unwrap();
        let error = write(
            &generation(&[("types.rs", "z")]),
            &dir.0,
            &WriteOptions::new("billing"),
        )
        .unwrap_err();
        assert!(
            matches!(&error, WriteError::OwnedByOther { manifest, .. } if manifest == Path::new(".roas-codegen-api-rust.json")),
            "{error}"
        );
    }

    #[test]
    fn unsafe_paths_and_labels_are_refused() {
        let dir = TempDir::new("unsafe");
        for path in ["../escape.rs", "/abs.rs", "a/../../b.rs", ""] {
            let error = write(
                &generation(&[(path, "x")]),
                &dir.0,
                &WriteOptions::new("api"),
            )
            .unwrap_err();
            assert!(
                matches!(error, WriteError::UnsafePath(_)),
                "{path}: {error}"
            );
        }
        for label in ["", "a b", "x/y", "..", "é"] {
            assert!(
                matches!(
                    manifest_name(label, Target::Go),
                    Err(WriteError::InvalidLabel(_))
                ),
                "{label}"
            );
        }
        assert_eq!(
            manifest_name("pet_store-2", Target::Go).unwrap(),
            ".roas-codegen-pet_store-2-go.json"
        );
    }

    #[test]
    fn check_reports_a_missing_manifest_and_missing_files() {
        let dir = TempDir::new("check");
        let g = generation(&[("types.rs", "a")]);
        let report = check(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.missing, vec![PathBuf::from("types.rs")]);
        assert!(report.manifest_outdated && !report.is_clean());
        fs::write(dir.0.join(".roas-codegen-api-rust.json"), "not json").unwrap();
        assert!(matches!(
            check(&g, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::Manifest { .. })
        ));
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(hash(b""), "fnv1a64:cbf29ce484222325");
        assert_eq!(hash(b"a"), "fnv1a64:af63dc4c8601ec8c");
    }

    fn manifest(dir: &Path, label: &str, files: &[(&str, &str)]) {
        let files: BTreeMap<String, String> = files
            .iter()
            .map(|(p, h)| ((*p).to_owned(), (*h).to_owned()))
            .collect();
        fs::write(
            dir.join(manifest_name(label, Target::Rust).unwrap()),
            manifest_text(&manifest_for(label, Target::Rust, "test.json", files)),
        )
        .unwrap();
    }

    #[test]
    fn a_manifest_entry_outside_the_directory_is_rejected_before_any_delete() {
        let root = TempDir::new("escape");
        let out = root.0.join("out");
        fs::create_dir_all(&out).unwrap();
        fs::write(root.0.join("victim.txt"), "precious").unwrap();
        for bad in ["../victim.txt", "/etc/passwd", "a/../../victim.txt", "./x"] {
            manifest(&out, "api", &[(bad, &hash(b"precious"))]);
            let error = write(
                &generation(&[("types.rs", "x")]),
                &out,
                &WriteOptions::new("api"),
            )
            .unwrap_err();
            assert!(
                matches!(error, WriteError::Manifest { .. }),
                "{bad}: {error}"
            );
            // Another target's manifest is held to the same rule.
            let error = write(
                &generation(&[("types.rs", "x")]),
                &out,
                &WriteOptions::new("other"),
            )
            .unwrap_err();
            assert!(
                matches!(error, WriteError::Manifest { .. }),
                "{bad}: {error}"
            );
        }
        assert_eq!(
            fs::read_to_string(root.0.join("victim.txt")).unwrap(),
            "precious"
        );
    }

    #[test]
    fn a_failed_run_does_not_block_the_next_one() {
        let dir = TempDir::new("recover");
        // A path whose parent is a file is caught before anything is written.
        fs::write(dir.0.join("b"), "file").unwrap();
        let g = generation(&[("a.rs", "a"), ("b/c.rs", "c")]);
        let error = write(&g, &dir.0, &WriteOptions::new("api")).unwrap_err();
        assert!(
            matches!(&error, WriteError::NotADirectory(p) if p == Path::new("b")),
            "{error}"
        );
        assert!(!dir.0.join("a.rs").exists(), "nothing written");
        // A failure after ownership was claimed: the next run owns the file.
        fs::remove_file(dir.0.join("b")).unwrap();
        manifest(
            &dir.0,
            "api",
            &[("a.rs", &hash(b"a")), ("b/c.rs", &hash(b"c"))],
        );
        fs::write(dir.0.join("a.rs"), "a").unwrap();
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(fs::read_to_string(dir.0.join("b/c.rs")).unwrap(), "c");
        assert!(
            check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn generated_paths_cannot_take_reserved_names() {
        let dir = TempDir::new("reserved");
        for path in [
            ".roas-codegen-api-rust.json",
            "sub/.roas-codegen-x-go.json",
            ".types.rs.roas-tmp-1",
        ] {
            let mut options = WriteOptions::new("api");
            options.overwrite_unowned = true;
            let error = write(&generation(&[(path, "x")]), &dir.0, &options).unwrap_err();
            assert!(
                matches!(error, WriteError::ReservedPath(_)),
                "{path}: {error}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn writes_never_follow_a_symlink() {
        let root = TempDir::new("symlink");
        let out = root.0.join("out");
        let outside = root.0.join("outside");
        fs::create_dir_all(&out).unwrap();
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, out.join("nested")).unwrap();
        let error = write(
            &generation(&[("nested/x.rs", "x")]),
            &out,
            &WriteOptions::new("api"),
        )
        .unwrap_err();
        assert!(
            matches!(&error, WriteError::Symlink(p) if p == Path::new("nested")),
            "{error}"
        );
        assert!(!outside.join("x.rs").exists());
    }

    #[test]
    fn errors_refuse_the_write_and_a_partial_write_keeps_old_files() {
        use crate::diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity};
        let dir = TempDir::new("partial");
        write(
            &generation(&[("types.rs", "a")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        let mut failed = generation(&[]);
        failed.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: DiagnosticKind::Ungeneratable {
                keyword: "not".into(),
            },
            schema_id: SchemaId::new(url::Url::parse("file:///x.json").unwrap(), ""),
            pointer: String::new(),
            message: String::new(),
        });
        let error = write(&failed, &dir.0, &WriteOptions::new("api")).unwrap_err();
        assert!(matches!(error, WriteError::HasErrors(1)), "{error}");
        let mut partial = WriteOptions::new("api");
        partial.allow_partial = true;
        let report = write(&failed, &dir.0, &partial).unwrap();
        assert!(report.removed.is_empty());
        assert_eq!(fs::read_to_string(dir.0.join("types.rs")).unwrap(), "a");
    }

    #[test]
    fn check_agrees_with_write_about_the_manifest() {
        let dir = TempDir::new("agree");
        let g = generation(&[("types.rs", "a")]);
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        let path = dir.0.join(".roas-codegen-api-rust.json");
        let text = fs::read_to_string(&path).unwrap();
        // Another writer version with identical output: nothing to do.
        fs::write(&path, text.replace(env!("CARGO_PKG_VERSION"), "0.0.0-old")).unwrap();
        assert!(
            check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        assert!(
            fs::read_to_string(&path).unwrap().contains("0.0.0-old"),
            "not rewritten"
        );
        // A real difference in ownership is still outdated.
        fs::write(&path, text.replace("fnv1a64:", "fnv1a64:0")).unwrap();
        assert!(
            !check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn a_newer_manifest_with_extra_fields_is_tolerated() {
        let dir = TempDir::new("newer");
        fs::write(
            dir.0.join(".roas-codegen-billing-rust.json"),
            r#"{"generator":"roas-codegen","version":"9.0.0","label":"billing","language":"rust","files":{"billing.rs":"fnv1a64:0"},"schema":2}"#,
        )
        .unwrap();
        write(
            &generation(&[("types.rs", "a")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn stale_removal_never_follows_a_symlink() {
        let root = TempDir::new("stale-symlink");
        let out = root.0.join("out");
        let outside = root.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        write(
            &generation(&[("nested/old.rs", "old")]),
            &out,
            &WriteOptions::new("api"),
        )
        .unwrap();
        fs::remove_dir_all(out.join("nested")).unwrap();
        fs::write(outside.join("old.rs"), "old").unwrap();
        std::os::unix::fs::symlink(&outside, out.join("nested")).unwrap();
        let error = write(
            &generation(&[("types.rs", "x")]),
            &out,
            &WriteOptions::new("api"),
        )
        .unwrap_err();
        assert!(matches!(error, WriteError::Symlink(_)), "{error}");
        assert_eq!(fs::read_to_string(outside.join("old.rs")).unwrap(), "old");
    }

    #[test]
    fn labels_are_lowercase_and_case_variants_never_take_over() {
        assert!(matches!(
            manifest_name("Pets", Target::Rust),
            Err(WriteError::InvalidLabel(_))
        ));
        let dir = TempDir::new("case");
        write(
            &generation(&[("pets_a.rs", "A")]),
            &dir.0,
            &WriteOptions::new("pets"),
        )
        .unwrap();
        // A manifest under a case variant of our name is ours where the
        // disk folds case, and another target's where it does not.
        let ours = dir.0.join(".roas-codegen-pets-rust.json");
        let text = fs::read_to_string(&ours).unwrap();
        fs::remove_file(&ours).unwrap();
        fs::write(dir.0.join(".roas-codegen-Pets-rust.json"), text).unwrap();
        let owners = other_owners(&dir.0, ".roas-codegen-pets-rust.json", []).unwrap();
        if ours.exists() {
            assert!(owners.is_empty(), "{owners:?}");
        } else {
            assert!(owners.contains_key("pets_a.rs"), "{owners:?}");
        }
    }

    #[test]
    fn check_agrees_with_a_partial_write() {
        use crate::diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity};
        let dir = TempDir::new("partial-check");
        write(
            &generation(&[("old.rs", "o"), ("keep.rs", "k")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        let mut failed = generation(&[("keep.rs", "k")]);
        failed.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: DiagnosticKind::Ungeneratable {
                keyword: "not".into(),
            },
            schema_id: SchemaId::new(url::Url::parse("file:///x.json").unwrap(), ""),
            pointer: String::new(),
            message: String::new(),
        });
        let mut partial = WriteOptions::new("api");
        partial.allow_partial = true;
        write(&failed, &dir.0, &partial).unwrap();
        assert!(dir.0.join("old.rs").exists());
        assert!(check(&failed, &dir.0, &partial).unwrap().is_clean());
        assert!(matches!(
            check(&failed, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::HasErrors(1))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn check_refuses_symlinks_as_write_does() {
        let root = TempDir::new("check-symlink");
        let out = root.0.join("out");
        let g = generation(&[("t.rs", "x")]);
        write(&g, &out, &WriteOptions::new("api")).unwrap();
        fs::write(root.0.join("elsewhere.rs"), "x").unwrap();
        fs::remove_file(out.join("t.rs")).unwrap();
        std::os::unix::fs::symlink(root.0.join("elsewhere.rs"), out.join("t.rs")).unwrap();
        assert!(matches!(
            check(&g, &out, &WriteOptions::new("api")),
            Err(WriteError::Symlink(_))
        ));
        assert!(matches!(
            write(&g, &out, &WriteOptions::new("api")),
            Err(WriteError::Symlink(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_temporary_symlink_is_never_followed() {
        let root = TempDir::new("tmp-symlink");
        let out = root.0.join("out");
        fs::create_dir_all(&out).unwrap();
        let victim = root.0.join("victim.txt");
        fs::write(&victim, "precious").unwrap();
        // Plant links under every name the old scheme would have used.
        for name in ["types.rs", ".roas-codegen-api-rust.json"] {
            std::os::unix::fs::symlink(
                &victim,
                out.join(format!("{}{}", tmp_prefix(name), std::process::id())),
            )
            .unwrap();
        }
        write(
            &generation(&[("types.rs", "generated")]),
            &out,
            &WriteOptions::new("api"),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
        assert_eq!(
            fs::read_to_string(out.join("types.rs")).unwrap(),
            "generated"
        );
        assert!(
            !fs::symlink_metadata(out.join("types.rs"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_output_directory_that_is_a_symlink_is_refused() {
        let root = TempDir::new("out-symlink");
        let real = root.0.join("real");
        fs::create_dir_all(&real).unwrap();
        let link = root.0.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let g = generation(&[("types.rs", "x")]);
        assert!(matches!(
            write(&g, &link, &WriteOptions::new("api")),
            Err(WriteError::OutputDirSymlink(_))
        ));
        assert!(matches!(
            check(&g, &link, &WriteOptions::new("api")),
            Err(WriteError::OutputDirSymlink(_))
        ));
        assert!(fs::read_dir(&real).unwrap().next().is_none());
    }

    #[test]
    fn a_label_owned_by_another_description_is_refused() {
        let dir = TempDir::new("source");
        let mut pets = WriteOptions::new("openapi");
        pets.source_id = Some("../pets/openapi".into());
        write(&generation(&[("openapi.rs", "pets")]), &dir.0, &pets).unwrap();
        let mut billing = WriteOptions::new("openapi");
        billing.source_id = Some("../billing/openapi".into());
        let g = generation(&[("openapi.rs", "billing")]);
        for result in [
            write(&g, &dir.0, &billing).map(|_| ()),
            check(&g, &dir.0, &billing).map(|_| ()),
        ] {
            let error = result.unwrap_err();
            assert!(
                matches!(&error, WriteError::SourceMismatch { previous, current, .. } if previous == "../pets/openapi" && current == "../billing/openapi"),
                "{error}"
            );
        }
        assert_eq!(
            fs::read_to_string(dir.0.join("openapi.rs")).unwrap(),
            "pets"
        );
        // A run without an identity keeps the recorded one.
        write(
            &generation(&[("openapi.rs", "pets2")]),
            &dir.0,
            &WriteOptions::new("openapi"),
        )
        .unwrap();
        assert!(
            fs::read_to_string(dir.0.join(".roas-codegen-openapi-rust.json"))
                .unwrap()
                .contains("../pets/openapi")
        );
    }

    #[test]
    fn one_run_at_a_time() {
        let dir = TempDir::new("lock");
        let lock = DirLock::acquire(&dir.0).unwrap();
        let error = write(
            &generation(&[("types.rs", "x")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap_err();
        assert!(matches!(error, WriteError::Locked(_)), "{error}");
        drop(lock);
        write(
            &generation(&[("types.rs", "x")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        // Released on every platform: it can be taken again.
        drop(DirLock::acquire(&dir.0).unwrap());
        let mut options = WriteOptions::new("api");
        options.overwrite_unowned = true;
        let error = write(&generation(&[(LOCK_FILE, "x")]), &dir.0, &options).unwrap_err();
        assert!(matches!(error, WriteError::ReservedPath(_)), "{error}");
    }

    #[test]
    fn ownership_is_compared_case_insensitively() {
        let dir = TempDir::new("case-paths");
        write(
            &generation(&[("Types.rs", "first")]),
            &dir.0,
            &WriteOptions::new("first"),
        )
        .unwrap();
        let mut options = WriteOptions::new("second");
        options.overwrite_unowned = true;
        let error = write(&generation(&[("types.rs", "second")]), &dir.0, &options).unwrap_err();
        assert!(matches!(error, WriteError::OwnedByOther { .. }), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_trailing_separator_does_not_bypass_the_symlink_check() {
        let root = TempDir::new("trailing");
        let real = root.0.join("real");
        fs::create_dir_all(&real).unwrap();
        let link = root.0.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let with_slash = PathBuf::from(format!("{}/", link.display()));
        let g = generation(&[("types.rs", "x")]);
        assert!(matches!(
            write(&g, &with_slash, &WriteOptions::new("api")),
            Err(WriteError::OutputDirSymlink(_))
        ));
        assert!(fs::read_dir(&real).unwrap().next().is_none());
    }

    #[test]
    fn a_moved_description_can_take_its_label_back() {
        let dir = TempDir::new("moved");
        let mut before = WriteOptions::new("pets");
        before.source_id = Some("../spec/pets".into());
        write(&generation(&[("pets.rs", "a")]), &dir.0, &before).unwrap();
        let mut after = WriteOptions::new("pets");
        after.source_id = Some("../api/pets".into());
        let error = write(&generation(&[("pets.rs", "b")]), &dir.0, &after).unwrap_err();
        assert!(
            matches!(error, WriteError::SourceMismatch { .. }),
            "{error}"
        );
        after.replace_source = true;
        write(&generation(&[("pets.rs", "b")]), &dir.0, &after).unwrap();
        assert_eq!(fs::read_to_string(dir.0.join("pets.rs")).unwrap(), "b");
        assert!(
            fs::read_to_string(dir.0.join(".roas-codegen-pets-rust.json"))
                .unwrap()
                .contains("../api/pets")
        );
        after.replace_source = false;
        assert!(
            check(&generation(&[("pets.rs", "b")]), &dir.0, &after)
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn conflicting_and_case_variant_paths_are_refused_up_front() {
        let dir = TempDir::new("conflicts");
        for files in [
            vec![("a", "x"), ("a/types.rs", "y")],
            vec![("Types.rs", "x"), ("types.rs", "y")],
            vec![("A", "x"), ("a/b.rs", "y")],
        ] {
            let error = write(&generation(&files), &dir.0, &WriteOptions::new("api")).unwrap_err();
            assert!(
                matches!(error, WriteError::ConflictingPaths(..)),
                "{files:?}: {error}"
            );
        }
        assert!(
            fs::read_dir(&dir.0).unwrap().next().is_none(),
            "nothing written"
        );
        let mut options = WriteOptions::new("api");
        options.overwrite_unowned = true;
        let error = write(
            &generation(&[(".ROAS-CODEGEN-api-rust.JSON", "x")]),
            &dir.0,
            &options,
        )
        .unwrap_err();
        assert!(matches!(error, WriteError::ReservedPath(_)), "{error}");
    }

    #[test]
    fn a_case_only_rename_needs_no_force_and_leaves_one_file() {
        let dir = TempDir::new("case-rename");
        write(
            &generation(&[("Types.rs", "one")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        let insensitive = same_file(&dir.0.join("Types.rs"), &dir.0.join("types.rs"));
        let next = generation(&[("types.rs", "two")]);
        let report = write(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        let names: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".rs"))
            .collect();
        assert_eq!(names, vec!["types.rs".to_owned()]);
        assert_eq!(fs::read_to_string(dir.0.join("types.rs")).unwrap(), "two");
        if insensitive {
            assert!(report.removed.is_empty(), "{report:?}");
        } else {
            assert_eq!(report.removed, vec![PathBuf::from("Types.rs")]);
        }
        assert!(
            check(&next, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn distinct_case_variants_on_a_case_sensitive_disk_are_stale() {
        let dir = TempDir::new("case-distinct");
        fs::write(dir.0.join("Probe.rs"), "").unwrap();
        if dir.0.join("probe.rs").exists() {
            return; // case-insensitive: covered above
        }
        fs::remove_file(dir.0.join("Probe.rs")).unwrap();
        write(
            &generation(&[("Types.rs", "x")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        let next = generation(&[("types.rs", "x")]);
        let report = check(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.stale, vec![PathBuf::from("Types.rs")]);
    }

    #[test]
    fn identical_unowned_files_are_taken_over() {
        let dir = TempDir::new("identical");
        fs::write(dir.0.join("types.rs"), "same").unwrap();
        let g = generation(&[("types.rs", "same")]);
        let report = write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.unchanged, vec![PathBuf::from("types.rs")]);
        assert!(
            check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
        fs::write(dir.0.join("other.rs"), "hand written").unwrap();
        assert!(matches!(
            write(
                &generation(&[("other.rs", "generated")]),
                &dir.0,
                &WriteOptions::new("api")
            ),
            Err(WriteError::Unowned(_))
        ));
    }

    #[test]
    fn reserved_names_are_refused_in_directories_too() {
        let dir = TempDir::new("reserved-dirs");
        for path in [
            ".roas-codegen-other-rust.json/types.rs",
            "a.roas-tmp-1/b.rs",
            ".roas-codegen.lock/x.rs",
        ] {
            let error = write(
                &generation(&[(path, "x")]),
                &dir.0,
                &WriteOptions::new("api"),
            )
            .unwrap_err();
            assert!(
                matches!(error, WriteError::ReservedPath(_)),
                "{path}: {error}"
            );
        }
    }

    #[test]
    fn the_manifest_is_named_after_the_generation_target() {
        let dir = TempDir::new("go-target");
        let g = Generation::new(
            Target::Go,
            vec![GeneratedFile::new("types.go", "package x")],
        );
        let report = write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.manifest, PathBuf::from(".roas-codegen-api-go.json"));
    }

    #[test]
    fn two_go_targets_never_share_a_package() {
        let dir = TempDir::new("go-package");
        let go =
            |name: &str| Generation::new(Target::Go, vec![GeneratedFile::new(name, "package x")]);
        write(&go("pets.go"), &dir.0, &WriteOptions::new("pets")).unwrap();
        // Rust beside it is fine, and so is the same target again.
        write(
            &generation(&[("pets.rs", "p")]),
            &dir.0,
            &WriteOptions::new("pets"),
        )
        .unwrap();
        write(&go("pets.go"), &dir.0, &WriteOptions::new("pets")).unwrap();
        for result in [
            write(&go("billing.go"), &dir.0, &WriteOptions::new("billing")).map(|_| ()),
            check(&go("billing.go"), &dir.0, &WriteOptions::new("billing")).map(|_| ()),
        ] {
            assert!(
                matches!(&result, Err(WriteError::SharedGoPackage { manifest }) if manifest == Path::new(".roas-codegen-pets-go.json")),
                "{result:?}"
            );
        }
        assert!(!dir.0.join("billing.go").exists());
        // In a directory of its own, it is a package of its own, whatever
        // directory its manifest is in, and the first is still fine.
        let nested =
            |name: &str| Generation::new(Target::Go, vec![GeneratedFile::new(name, "package x")]);
        write(
            &nested("billing/billing.go"),
            &dir.0,
            &WriteOptions::new("billing"),
        )
        .unwrap();
        write(&go("pets.go"), &dir.0, &WriteOptions::new("pets")).unwrap();
        check(&go("pets.go"), &dir.0, &WriteOptions::new("pets")).unwrap();
        // Two manifests side by side, files together in a subdirectory.
        let result = write(
            &nested("billing/more.go"),
            &dir.0,
            &WriteOptions::new("more"),
        );
        assert!(
            matches!(&result, Err(WriteError::SharedGoPackage { manifest }) if manifest == Path::new(".roas-codegen-billing-go.json")),
            "{result:?}"
        );
    }

    #[test]
    fn check_refuses_what_write_refuses() {
        let dir = TempDir::new("check-refuses");
        fs::write(dir.0.join("types.rs"), "hand written").unwrap();
        let g = generation(&[("types.rs", "generated")]);
        assert!(matches!(
            check(&g, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::Unowned(_))
        ));
        let mut forced = WriteOptions::new("api");
        forced.overwrite_unowned = true;
        assert!(!check(&g, &dir.0, &forced).unwrap().is_clean());
        write(
            &generation(&[("pets.rs", "p")]),
            &dir.0,
            &WriteOptions::new("pets"),
        )
        .unwrap();
        assert!(matches!(
            check(
                &generation(&[("pets.rs", "p")]),
                &dir.0,
                &WriteOptions::new("api")
            ),
            Err(WriteError::OwnedByOther { .. })
        ));
    }

    #[test]
    fn an_interrupted_rewrite_still_lets_the_old_content_go_stale() {
        let dir = TempDir::new("interrupted");
        write(
            &generation(&[("old.rs", "v1"), ("keep.rs", "k")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        // A run that would write v2 stops after its pending manifest.
        let manifest = dir.0.join(".roas-codegen-api-rust.json");
        let mut pending = read_manifest(&manifest).unwrap().unwrap();
        pending
            .replacing
            .insert("old.rs".into(), pending.files["old.rs"].clone());
        pending.files.insert("old.rs".into(), hash(b"v2"));
        fs::write(&manifest, manifest_text(&pending)).unwrap();

        let next = generation(&[("keep.rs", "k")]);
        assert_eq!(
            check(&next, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .stale,
            vec![PathBuf::from("old.rs")]
        );
        let report = write(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.removed, vec![PathBuf::from("old.rs")]);
        assert!(report.kept_edited.is_empty());
        let finished = read_manifest(&manifest).unwrap().unwrap();
        assert!(finished.replacing.is_empty());
        assert!(
            check(&next, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn an_unchanged_case_only_rename_takes_the_new_spelling() {
        let dir = TempDir::new("respell");
        write(
            &generation(&[("Types.rs", "same")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        write(
            &generation(&[("types.rs", "same")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        let names: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".rs"))
            .collect();
        assert_eq!(names, vec!["types.rs".to_owned()]);
    }

    #[test]
    fn non_ascii_generated_paths_are_refused() {
        let dir = TempDir::new("non-ascii");
        for path in ["\u{c4}.rs", "sub/e\u{301}.rs"] {
            assert!(
                matches!(
                    write(
                        &generation(&[(path, "x")]),
                        &dir.0,
                        &WriteOptions::new("api")
                    ),
                    Err(WriteError::NonAsciiPath(_))
                ),
                "{path}"
            );
        }
    }

    #[test]
    fn manifests_stored_in_another_case_still_own_their_files() {
        let dir = TempDir::new("manifest-case");
        write(
            &generation(&[("a.rs", "a")]),
            &dir.0,
            &WriteOptions::new("one"),
        )
        .unwrap();
        fs::rename(
            dir.0.join(".roas-codegen-one-rust.json"),
            dir.0.join(".ROAS-CODEGEN-one-rust.JSON"),
        )
        .unwrap();
        assert!(matches!(
            write(
                &generation(&[("a.rs", "a")]),
                &dir.0,
                &WriteOptions::new("two")
            ),
            Err(WriteError::OwnedByOther { .. })
        ));
    }

    #[test]
    fn an_own_stale_file_gives_way_to_a_directory() {
        let dir = TempDir::new("file-to-dir");
        write(
            &generation(&[("models", "m")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        let next = generation(&[("models/x.rs", "x")]);
        let report = check(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.stale, vec![PathBuf::from("models")]);
        let report = write(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.removed, vec![PathBuf::from("models")]);
        assert_eq!(fs::read_to_string(dir.0.join("models/x.rs")).unwrap(), "x");
        // Edited, it stays in the way.
        write(
            &generation(&[("other", "o")]),
            &dir.0,
            &WriteOptions::new("b"),
        )
        .unwrap();
        fs::write(dir.0.join("other"), "edited").unwrap();
        let mut forced = WriteOptions::new("b");
        forced.overwrite_unowned = true;
        assert!(matches!(
            write(&generation(&[("other/y.rs", "y")]), &dir.0, &forced),
            Err(WriteError::NotADirectory(_))
        ));
    }

    #[test]
    fn an_interrupted_respelling_is_still_recognised() {
        let dir = TempDir::new("respell-interrupted");
        let manifest = dir.0.join(".roas-codegen-api-rust.json");
        write(
            &generation(&[("Types.rs", "old")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        if !same_file(&dir.0.join("Types.rs"), &dir.0.join("types.rs")) {
            return; // case-sensitive: the spellings are separate files
        }
        // Stop the rename to types.rs before its content is written: the
        // pending manifest is written, then the run fails on the file.
        let failed = {
            let mut m = read_manifest(&manifest).unwrap().unwrap();
            m.files.insert("types.rs".into(), hash(b"new"));
            m.replacing.insert("types.rs".into(), hash(b"old"));
            fs::write(&manifest, manifest_text(&m)).unwrap();
            m
        };
        assert!(failed.wrote("types.rs", b"old"));
        let report = write(
            &generation(&[("keep.rs", "k")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        assert!(report.kept_edited.is_empty(), "{report:?}");
        assert!(!dir.0.join("Types.rs").exists());
    }

    #[test]
    fn another_targets_directories_and_files_are_not_replaced() {
        let dir = TempDir::new("other-tree");
        write(
            &generation(&[("nested/a.rs", "a")]),
            &dir.0,
            &WriteOptions::new("one"),
        )
        .unwrap();
        fs::remove_dir_all(dir.0.join("nested")).unwrap();
        let mut forced = WriteOptions::new("two");
        forced.overwrite_unowned = true;
        for path in ["nested", "nested/a.rs/b.rs"] {
            assert!(
                matches!(
                    write(&generation(&[(path, "x")]), &dir.0, &forced),
                    Err(WriteError::OwnedByOther { .. })
                ),
                "{path}"
            );
        }
        write(&generation(&[("nested2.rs", "x")]), &dir.0, &forced).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_retried_interrupted_run_remembers_what_is_on_disk() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("retry");
        let manifest = dir.0.join(".roas-codegen-api-rust.json");
        let opts = WriteOptions::new("api");
        write(
            &generation(&[("sub/x.rs", "v1"), ("keep.rs", "k")]),
            &dir.0,
            &opts,
        )
        .unwrap();
        // Run 1 wrote v2 and stopped before its final manifest.
        fs::write(dir.0.join("sub/x.rs"), "v2").unwrap();
        let mut pending = read_manifest(&manifest).unwrap().unwrap();
        pending.replacing.insert("sub/x.rs".into(), hash(b"v1"));
        pending.files.insert("sub/x.rs".into(), hash(b"v2"));
        fs::write(&manifest, manifest_text(&pending)).unwrap();
        // Run 2 plans v3 and fails before writing it.
        let sub = dir.0.join("sub");
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o555)).unwrap();
        if fs::write(sub.join("probe"), "").is_ok() {
            fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
            return; // running as root: permissions do not stop it
        }
        let failed = write(
            &generation(&[("sub/x.rs", "v3"), ("keep.rs", "k")]),
            &dir.0,
            &opts,
        );
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(failed.is_err(), "{failed:?}");
        assert_eq!(fs::read_to_string(sub.join("x.rs")).unwrap(), "v2");
        // Run 3 no longer generates it: still ours, so removed.
        let report = write(&generation(&[("keep.rs", "k")]), &dir.0, &opts).unwrap();
        assert_eq!(report.removed, vec![PathBuf::from("sub/x.rs")]);
        assert!(report.kept_edited.is_empty());
    }

    #[test]
    fn manifests_naming_the_generators_own_files_are_refused() {
        let dir = TempDir::new("reserved-entry");
        let manifest = dir.0.join(".roas-codegen-api-rust.json");
        let mut bad = manifest_for("api", Target::Rust, "", BTreeMap::new());
        bad.files.insert(LOCK_FILE.into(), hash(b""));
        fs::write(&manifest, manifest_text(&bad)).unwrap();
        assert!(matches!(
            write(
                &generation(&[("a.rs", "a")]),
                &dir.0,
                &WriteOptions::new("api")
            ),
            Err(WriteError::Manifest { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_link_under_another_name_is_not_ours() {
        let dir = TempDir::new("hard-link");
        write(
            &generation(&[("old.rs", "v1")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        fs::hard_link(dir.0.join("old.rs"), dir.0.join("manual.rs")).unwrap();
        assert!(matches!(
            write(
                &generation(&[("manual.rs", "v2")]),
                &dir.0,
                &WriteOptions::new("api")
            ),
            Err(WriteError::Unowned(_))
        ));
        assert_eq!(fs::read_to_string(dir.0.join("manual.rs")).unwrap(), "v1");
    }

    #[test]
    fn a_recorded_file_that_became_a_directory_is_dropped() {
        let dir = TempDir::new("became-dir");
        let manifest = dir.0.join(".roas-codegen-api-rust.json");
        write(
            &generation(&[("models", "m")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        // Interrupted after models became a directory, before the final
        // manifest: the pending one still records both.
        fs::remove_file(dir.0.join("models")).unwrap();
        fs::create_dir(dir.0.join("models")).unwrap();
        fs::write(dir.0.join("models/x.rs"), "x").unwrap();
        let mut pending = read_manifest(&manifest).unwrap().unwrap();
        pending.files.insert("models/x.rs".into(), hash(b"x"));
        fs::write(&manifest, manifest_text(&pending)).unwrap();
        let next = generation(&[("models/x.rs", "x")]);
        let report = check(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert!(
            report.stale.is_empty() && report.stale_edited.is_empty(),
            "{report:?}"
        );
        write(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert!(
            check(&next, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
        assert!(dir.0.join("models/x.rs").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_stale_file_fails_check_as_it_fails_write() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("unreadable");
        write(
            &generation(&[("a.rs", "a"), ("b.rs", "b")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        let b = dir.0.join("b.rs");
        fs::set_permissions(&b, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(&b).is_ok() {
            return; // running as root
        }
        let next = generation(&[("a.rs", "a")]);
        let checked = check(&next, &dir.0, &WriteOptions::new("api"));
        let written = write(&next, &dir.0, &WriteOptions::new("api"));
        fs::set_permissions(&b, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(checked, Err(WriteError::Io { .. })), "{checked:?}");
        assert!(matches!(written, Err(WriteError::Io { .. })), "{written:?}");
    }

    #[test]
    fn a_case_variant_stale_file_gives_way_to_a_directory() {
        let dir = TempDir::new("case-blocker");
        write(
            &generation(&[("Models", "m")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        if !same_file(&dir.0.join("Models"), &dir.0.join("models")) {
            return; // case-sensitive: no blocker
        }
        let next = generation(&[("models/x.rs", "x")]);
        write(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert!(
            check(&next, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn a_lock_file_left_by_a_killed_run_does_not_block() {
        let dir = TempDir::new("stale-lock");
        fs::write(dir.0.join(LOCK_FILE), "").unwrap();
        write(
            &generation(&[("a.rs", "a")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        drop(DirLock::acquire(&dir.0).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn an_interrupted_file_to_directory_change_with_the_file_still_there_recovers() {
        let dir = TempDir::new("blocked-retry");
        let manifest = dir.0.join(".roas-codegen-api-rust.json");
        write(
            &generation(&[("sub/models", "m")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        // The pending manifest of a run that then could not remove the file.
        let mut pending = read_manifest(&manifest).unwrap().unwrap();
        pending.files.insert("sub/models/x.rs".into(), hash(b"x"));
        fs::write(&manifest, manifest_text(&pending)).unwrap();
        let next = generation(&[("sub/models/x.rs", "x")]);
        assert_eq!(
            check(&next, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .stale,
            vec![PathBuf::from("sub/models")]
        );
        write(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert!(
            check(&next, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn an_interrupted_respelling_is_removed_and_reported_once() {
        let dir = TempDir::new("respell-once");
        let manifest = dir.0.join(".roas-codegen-api-rust.json");
        write(
            &generation(&[("Types.rs", "old")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        if !same_file(&dir.0.join("Types.rs"), &dir.0.join("types.rs")) {
            return;
        }
        // Respelled and rewritten, then stopped before the final manifest.
        fs::remove_file(dir.0.join("Types.rs")).unwrap();
        fs::write(dir.0.join("types.rs"), "new").unwrap();
        let mut pending = read_manifest(&manifest).unwrap().unwrap();
        pending.files.insert("types.rs".into(), hash(b"new"));
        pending.replacing.insert("types.rs".into(), hash(b"old"));
        fs::write(&manifest, manifest_text(&pending)).unwrap();
        let next = generation(&[("keep.rs", "k")]);
        let checked = check(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(checked.stale.len(), 1, "{checked:?}");
        assert!(checked.stale_edited.is_empty(), "{checked:?}");
        let report = write(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.removed.len(), 1, "{report:?}");
        assert!(report.kept_edited.is_empty(), "{report:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_link_differing_only_in_case_is_not_a_respelling() {
        let dir = TempDir::new("hard-link-case");
        write(
            &generation(&[("Foo.rs", "v1")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        if same_file(&dir.0.join("Foo.rs"), &dir.0.join("foo.rs")) {
            return; // case-insensitive: no second entry is possible
        }
        fs::hard_link(dir.0.join("Foo.rs"), dir.0.join("foo.rs")).unwrap();
        let mut forced = WriteOptions::new("api");
        forced.overwrite_unowned = true;
        write(&generation(&[("foo.rs", "v2")]), &dir.0, &forced).unwrap();
        assert!(!dir.0.join("Foo.rs").exists());
        assert_eq!(fs::read_to_string(dir.0.join("foo.rs")).unwrap(), "v2");
        let leftovers: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(TMP_MARKER))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_link_replacing_another_generated_file_is_kept() {
        let dir = TempDir::new("link-stale");
        write(
            &generation(&[("a.rs", "a"), ("b.rs", "b"), ("c.rs", "c")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        fs::remove_file(dir.0.join("b.rs")).unwrap();
        fs::hard_link(dir.0.join("a.rs"), dir.0.join("b.rs")).unwrap();
        let next = generation(&[("c.rs", "c")]);
        let checked = check(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(checked.stale, vec![PathBuf::from("a.rs")]);
        assert_eq!(checked.stale_edited, vec![PathBuf::from("b.rs")]);
        let report = write(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.removed, vec![PathBuf::from("a.rs")]);
        assert_eq!(report.kept_edited, vec![PathBuf::from("b.rs")]);
        assert_eq!(fs::read_to_string(dir.0.join("b.rs")).unwrap(), "a");
    }

    #[cfg(unix)]
    #[test]
    fn manifests_are_never_read_through_links_or_from_non_files() {
        let dir = TempDir::new("manifest-link");
        let target = dir.0.join("elsewhere.json");
        fs::write(&target, "{}").unwrap();
        std::os::unix::fs::symlink(&target, dir.0.join(".roas-codegen-other-rust.json")).unwrap();
        let g = generation(&[("a.rs", "a")]);
        assert!(matches!(
            write(&g, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::Symlink(_))
        ));
        assert!(matches!(
            check(&g, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::Symlink(_))
        ));
        fs::remove_file(dir.0.join(".roas-codegen-other-rust.json")).unwrap();
        fs::create_dir(dir.0.join(".roas-codegen-other-rust.json")).unwrap();
        assert!(matches!(
            write(&g, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::Manifest { .. })
        ));
    }

    #[test]
    fn an_empty_output_path_is_the_current_directory() {
        assert_eq!(current_if_empty(Path::new("")), Path::new("."));
        assert_eq!(current_if_empty(Path::new("out")), Path::new("out"));
        assert_eq!(current_if_empty(Path::new("out/.")), Path::new("out"));
        assert_eq!(current_if_empty(Path::new("./")), Path::new("."));
    }

    #[test]
    fn temporary_files_left_by_a_killed_run_are_removed() {
        let dir = TempDir::new("leftover-tmp");
        fs::create_dir_all(dir.0.join("sub")).unwrap();
        let top = dir.0.join(format!("{}1-2-3", tmp_prefix("a.rs")));
        let nested = dir.0.join(format!("sub/{}1-2-3", tmp_prefix("b.rs")));
        fs::write(&top, "partial").unwrap();
        fs::write(&nested, "partial").unwrap();
        fs::write(dir.0.join("sub/keep.txt"), "mine").unwrap();
        write(
            &generation(&[("a.rs", "a"), ("sub/b.rs", "b")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        assert!(!top.exists() && !nested.exists());
        assert!(dir.0.join("sub/keep.txt").exists());
    }

    #[test]
    fn rejected_paths_never_lead_the_temp_cleanup_outside() {
        let root = TempDir::new("cleanup-outside");
        let out = root.0.join("out");
        let outside = root.0.join("x");
        fs::create_dir_all(&out).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let victim = outside.join(format!("{}1-2-3", tmp_prefix("a.rs")));
        fs::write(&victim, "not ours").unwrap();
        assert!(matches!(
            write(
                &generation(&[("../x/a.rs", "a")]),
                &out,
                &WriteOptions::new("api")
            ),
            Err(WriteError::UnsafePath(_))
        ));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, out.join("link")).unwrap();
            assert!(matches!(
                write(
                    &generation(&[("link/a.rs", "a")]),
                    &out,
                    &WriteOptions::new("api")
                ),
                Err(WriteError::Symlink(_))
            ));
        }
        assert!(victim.exists());
    }

    #[test]
    fn a_refused_write_creates_no_directory() {
        let root = TempDir::new("no-create");
        let out = root.0.join("new/dir");
        assert!(
            write(
                &generation(&[("../a.rs", "a")]),
                &out,
                &WriteOptions::new("api")
            )
            .is_err()
        );
        assert!(!out.exists());
    }

    #[test]
    fn a_directory_where_a_file_goes_is_named_as_such() {
        let dir = TempDir::new("dir-in-way");
        fs::create_dir(dir.0.join("a.rs")).unwrap();
        let error = write(
            &generation(&[("a.rs", "a")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap_err();
        assert!(matches!(error, WriteError::IsADirectory(_)), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_linked_old_spelling_is_replaced_not_refused() {
        let dir = TempDir::new("linked-respell");
        write(
            &generation(&[("Foo.rs", "v1")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        if !same_file(&dir.0.join("Foo.rs"), &dir.0.join("foo.rs")) {
            return;
        }
        let backup = dir.0.join("backup");
        fs::hard_link(dir.0.join("Foo.rs"), &backup).unwrap();
        let next = generation(&[("foo.rs", "v2")]);
        let mut opts = WriteOptions::new("api");
        opts.overwrite_unowned = false;
        assert!(check(&next, &dir.0, &opts).is_ok());
        write(&next, &dir.0, &opts).unwrap();
        assert_eq!(fs::read_to_string(dir.0.join("foo.rs")).unwrap(), "v2");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "v1");
    }

    #[test]
    fn a_lock_path_that_is_no_file_fails_check_as_it_fails_write() {
        let dir = TempDir::new("bad-lock");
        fs::create_dir(dir.0.join(LOCK_FILE)).unwrap();
        let g = generation(&[("a.rs", "a")]);
        assert!(matches!(
            check(&g, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::BadLock(_))
        ));
        assert!(matches!(
            write(&g, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::BadLock(_))
        ));
    }

    #[test]
    fn a_stale_file_another_manifest_lists_is_kept() {
        let dir = TempDir::new("co-owned");
        write(
            &generation(&[("x.rs", "x"), ("y.rs", "y")]),
            &dir.0,
            &WriteOptions::new("aaa"),
        )
        .unwrap();
        let mut other = manifest_for("bbb", Target::Rust, "", BTreeMap::new());
        other.files.insert("x.rs".into(), hash(b"x"));
        fs::write(
            dir.0.join(".roas-codegen-bbb-rust.json"),
            manifest_text(&other),
        )
        .unwrap();
        let next = generation(&[("y.rs", "y")]);
        let checked = check(&next, &dir.0, &WriteOptions::new("aaa")).unwrap();
        assert!(checked.stale.is_empty(), "{checked:?}");
        let report = write(&next, &dir.0, &WriteOptions::new("aaa")).unwrap();
        assert!(report.removed.is_empty(), "{report:?}");
        assert!(dir.0.join("x.rs").exists());
    }

    #[cfg(unix)]
    #[test]
    fn check_lists_a_file_rewritten_through_a_linked_old_spelling() {
        let dir = TempDir::new("linked-check");
        write(
            &generation(&[("Foo.rs", "same")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        if !same_file(&dir.0.join("Foo.rs"), &dir.0.join("foo.rs")) {
            return;
        }
        fs::hard_link(dir.0.join("Foo.rs"), dir.0.join("backup")).unwrap();
        let next = generation(&[("foo.rs", "same")]);
        let checked = check(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(checked.missing, vec![PathBuf::from("foo.rs")]);
        let report = write(&next, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.written, vec![PathBuf::from("foo.rs")]);
    }

    #[test]
    fn a_taken_over_file_gets_the_planned_spelling() {
        let dir = TempDir::new("takeover-spelling");
        fs::write(dir.0.join("A.rs"), "same").unwrap();
        if !same_file(&dir.0.join("A.rs"), &dir.0.join("a.rs")) {
            return;
        }
        let g = generation(&[("a.rs", "same")]);
        assert_eq!(
            check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .changed,
            vec![PathBuf::from("a.rs")]
        );
        let report = write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        assert_eq!(
            report.renamed,
            vec![(PathBuf::from("A.rs"), PathBuf::from("a.rs"))]
        );
        let names: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".rs"))
            .collect();
        assert_eq!(names, vec!["a.rs".to_owned()]);
        assert!(
            check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn a_shared_stale_file_is_not_reported_as_edited() {
        let dir = TempDir::new("shared-report");
        write(
            &generation(&[("x.rs", "x"), ("y.rs", "y")]),
            &dir.0,
            &WriteOptions::new("aaa"),
        )
        .unwrap();
        let mut other = manifest_for("bbb", Target::Rust, "", BTreeMap::new());
        other.files.insert("x.rs".into(), hash(b"x"));
        fs::write(
            dir.0.join(".roas-codegen-bbb-rust.json"),
            manifest_text(&other),
        )
        .unwrap();
        let next = generation(&[("y.rs", "y")]);
        assert!(
            check(&next, &dir.0, &WriteOptions::new("aaa"))
                .unwrap()
                .stale_edited
                .is_empty()
        );
        assert!(
            write(&next, &dir.0, &WriteOptions::new("aaa"))
                .unwrap()
                .kept_edited
                .is_empty()
        );
    }

    #[test]
    fn an_obsolete_record_behind_a_file_does_not_block() {
        let dir = TempDir::new("obsolete-behind-file");
        let manifest = dir.0.join(".roas-codegen-api-rust.json");
        write(
            &generation(&[("models", "m")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        // Pending manifest of a run towards models/x.rs that stopped early.
        let mut pending = read_manifest(&manifest).unwrap().unwrap();
        pending.files.insert("models/x.rs".into(), hash(b"x"));
        fs::write(&manifest, manifest_text(&pending)).unwrap();
        let back = generation(&[("models", "m")]);
        assert!(
            !check(&back, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
        write(&back, &dir.0, &WriteOptions::new("api")).unwrap();
        assert!(
            check(&back, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
        assert_eq!(fs::read_to_string(dir.0.join("models")).unwrap(), "m");
    }

    #[test]
    fn another_runs_temporaries_in_a_nested_directory_are_left_alone() {
        let dir = TempDir::new("nested-temps");
        fs::create_dir_all(dir.0.join("nested")).unwrap();
        let theirs = dir
            .0
            .join(format!("nested/{}1-2-3", tmp_prefix("other.rs")));
        fs::write(&theirs, "in flight").unwrap();
        write(
            &generation(&[("nested/big.rs", "b")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        assert!(theirs.exists());
    }

    #[test]
    fn a_name_near_the_length_limit_can_be_written() {
        let dir = TempDir::new("long-name");
        let name = format!("{}.rs", "a".repeat(250));
        write(
            &generation(&[(name.as_str(), "x")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dir.0.join(&name)).unwrap(), "x");
    }

    #[test]
    fn a_nested_roots_files_are_not_adopted_from_above() {
        let dir = TempDir::new("nested-roots");
        let nested = dir.0.join("nested");
        write(
            &generation(&[("types.rs", "t")]),
            &nested,
            &WriteOptions::new("inner"),
        )
        .unwrap();
        let outer = generation(&[("nested/types.rs", "t")]);
        assert!(matches!(
            write(&outer, &dir.0, &WriteOptions::new("outer")),
            Err(WriteError::OwnedByOther { .. })
        ));
        assert!(matches!(
            check(&outer, &dir.0, &WriteOptions::new("outer")),
            Err(WriteError::OwnedByOther { .. })
        ));
    }

    #[test]
    fn an_enclosing_roots_files_are_not_adopted_from_below() {
        let dir = TempDir::new("enclosing-root");
        write(
            &generation(&[("nested/types.rs", "t")]),
            &dir.0,
            &WriteOptions::new("outer"),
        )
        .unwrap();
        let inner = generation(&[("types.rs", "t")]);
        assert!(matches!(
            write(&inner, &dir.0.join("nested"), &WriteOptions::new("inner")),
            Err(WriteError::OwnedByOther { .. })
        ));
    }

    #[test]
    fn the_output_directory_itself_has_its_own_errors() {
        let dir = TempDir::new("out-file");
        let file = dir.0.join("afile");
        fs::write(&file, "").unwrap();
        assert!(matches!(
            write(
                &generation(&[("a.rs", "a")]),
                &file,
                &WriteOptions::new("api")
            ),
            Err(WriteError::OutputDirNotADirectory(_))
        ));
    }

    #[test]
    fn nested_roots_lock_each_other() {
        let dir = TempDir::new("nested-locks");
        // A run rooted at `nested` holds its lock; one rooted above that
        // writes into `nested` must not run alongside it.
        fs::create_dir_all(dir.0.join("nested")).unwrap();
        let held = DirLock::acquire(&dir.0.join("nested")).unwrap();
        assert!(matches!(
            write(
                &generation(&[("nested/types.rs", "t")]),
                &dir.0,
                &WriteOptions::new("outer")
            ),
            Err(WriteError::Locked(_))
        ));
        drop(held);
        // And the other way round: an enclosing root with a manifest.
        write(
            &generation(&[("other.rs", "o")]),
            &dir.0,
            &WriteOptions::new("outer"),
        )
        .unwrap();
        let held = DirLock::acquire(&dir.0).unwrap();
        assert!(matches!(
            write(
                &generation(&[("types.rs", "t")]),
                &dir.0.join("nested"),
                &WriteOptions::new("inner")
            ),
            Err(WriteError::Locked(_))
        ));
        drop(held);
    }

    #[test]
    fn a_refused_write_leaves_no_directories_it_created() {
        let dir = TempDir::new("refused-dirs");
        write(
            &generation(&[("inner/api.rs", "a")]),
            &dir.0,
            &WriteOptions::new("outer"),
        )
        .unwrap();
        fs::remove_dir_all(dir.0.join("inner")).unwrap();
        let inner = dir.0.join("inner");
        let g = generation(&[("api.rs", "a"), ("deep/x.rs", "x")]);
        assert!(matches!(
            check(&g, &inner, &WriteOptions::new("inner")),
            Err(WriteError::OwnedByOther { .. })
        ));
        assert!(matches!(
            write(&g, &inner, &WriteOptions::new("inner")),
            Err(WriteError::OwnedByOther { .. })
        ));
        // Elsewhere the lock file it made stays, and the directory with it.
        #[cfg(unix)]
        assert!(!inner.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_manifest_in_a_nested_directory_is_named_with_its_directory() {
        let dir = TempDir::new("nested-manifest-link");
        write(
            &generation(&[("inner/x.rs", "x")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        let target = dir.0.join("elsewhere.json");
        fs::write(&target, "{}").unwrap();
        std::os::unix::fs::symlink(&target, dir.0.join("inner/.roas-codegen-n-rust.json")).unwrap();
        let error = write(
            &generation(&[("inner/x.rs", "x")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap_err();
        assert!(
            matches!(&error, WriteError::Symlink(p) if p == Path::new("inner/.roas-codegen-n-rust.json")),
            "{error}"
        );
    }

    #[test]
    fn sibling_roots_under_one_enclosing_root_run_side_by_side() {
        let dir = TempDir::new("siblings");
        write(
            &generation(&[("top.rs", "t")]),
            &dir.0,
            &WriteOptions::new("outer"),
        )
        .unwrap();
        // A sibling's run holds its shared lock on the enclosing root.
        let sibling = DirLock::share(&dir.0, true).unwrap();
        write(
            &generation(&[("b.rs", "b")]),
            &dir.0.join("b"),
            &WriteOptions::new("b"),
        )
        .unwrap();
        // The enclosing root's own run is kept out meanwhile.
        assert!(matches!(
            write(
                &generation(&[("top.rs", "t")]),
                &dir.0,
                &WriteOptions::new("outer")
            ),
            Err(WriteError::Locked(_))
        ));
        drop(sibling);
    }

    #[test]
    fn one_directory_spelled_two_ways_is_refused() {
        let dir = TempDir::new("dir-spellings");
        assert!(matches!(
            write(
                &generation(&[("models/a.rs", "a"), ("Models/b.rs", "b")]),
                &dir.0,
                &WriteOptions::new("api")
            ),
            Err(WriteError::ConflictingPaths(..))
        ));
    }

    #[test]
    fn an_output_path_through_a_missing_directory_and_dot_dot_works() {
        let dir = TempDir::new("dot-dot");
        let out = dir.0.join("scratch/../generated");
        write(
            &generation(&[("a.rs", "a")]),
            &out,
            &WriteOptions::new("api"),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(dir.0.join("generated/a.rs")).unwrap(),
            "a"
        );
    }

    #[test]
    fn a_refused_run_never_removes_a_lock_another_run_holds() {
        let dir = TempDir::new("lock-race");
        let gen_dir = dir.0.join("gen");
        let mut loser = Created {
            dirs: Vec::new(),
            keep: false,
            lock: None,
            shared: Vec::new(),
        };
        loser.dir_all(&gen_dir).unwrap();
        // Another run wins the lock in the directory the loser created.
        let winner = DirLock::acquire(&gen_dir).unwrap();
        assert!(matches!(
            DirLock::acquire(&gen_dir),
            Err(WriteError::Locked(_))
        ));
        drop(loser);
        assert!(gen_dir.join(LOCK_FILE).exists());
        // Still the winner's: nobody else can take it.
        assert!(matches!(
            DirLock::acquire(&gen_dir),
            Err(WriteError::Locked(_))
        ));
        drop(winner);
    }

    #[test]
    fn check_sees_enclosing_owners_through_dot_dot_and_a_missing_directory() {
        let dir = TempDir::new("dot-dot-check");
        write(
            &generation(&[("gen/a.rs", "a")]),
            &dir.0,
            &WriteOptions::new("outer"),
        )
        .unwrap();
        fs::remove_dir_all(dir.0.join("gen")).unwrap();
        let out = dir.0.join("scratch/../gen");
        let g = generation(&[("a.rs", "a")]);
        assert!(matches!(
            check(&g, &out, &WriteOptions::new("inner")),
            Err(WriteError::OwnedByOther { .. })
        ));
        assert!(matches!(
            write(&g, &out, &WriteOptions::new("inner")),
            Err(WriteError::OwnedByOther { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_enclosing_lock_file_still_shares() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("ro-lock");
        write(
            &generation(&[("top.rs", "t")]),
            &dir.0,
            &WriteOptions::new("outer"),
        )
        .unwrap();
        let lock = dir.0.join(LOCK_FILE);
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o444)).unwrap();
        let result = write(
            &generation(&[("b.rs", "b")]),
            &dir.0.join("b"),
            &WriteOptions::new("b"),
        );
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        result.unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_own_lock_file_still_locks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("ro-own-lock");
        let g = generation(&[("a.rs", "a")]);
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        let lock = dir.0.join(LOCK_FILE);
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o444)).unwrap();
        let result = write(&g, &dir.0, &WriteOptions::new("api"));
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        result.unwrap();
    }

    #[test]
    fn an_output_directory_owned_as_a_file_above_is_refused() {
        let dir = TempDir::new("owned-as-file");
        write(
            &generation(&[("nested", "n")]),
            &dir.0,
            &WriteOptions::new("outer"),
        )
        .unwrap();
        fs::remove_file(dir.0.join("nested")).unwrap();
        let g = generation(&[("a.rs", "a")]);
        let inner = dir.0.join("nested");
        assert!(matches!(
            check(&g, &inner, &WriteOptions::new("inner")),
            Err(WriteError::OutputDirOwned { .. })
        ));
        assert!(matches!(
            write(&g, &inner, &WriteOptions::new("inner")),
            Err(WriteError::OutputDirOwned { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_enclosing_lock_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("unreadable-lock");
        write(
            &generation(&[("top.rs", "t")]),
            &dir.0,
            &WriteOptions::new("outer"),
        )
        .unwrap();
        let lock = dir.0.join(LOCK_FILE);
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::File::open(&lock).is_ok() {
            fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
            return; // running as root
        }
        let result = write(
            &generation(&[("b.rs", "b")]),
            &dir.0.join("b"),
            &WriteOptions::new("b"),
        );
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(result, Err(WriteError::Io { .. })), "{result:?}");
    }

    #[test]
    fn names_windows_would_shorten_are_refused() {
        let dir = TempDir::new("trailing-dot");
        for path in ["a.rs.", "a.rs ", "sub./a.rs"] {
            assert!(
                matches!(
                    write(
                        &generation(&[(path, "x")]),
                        &dir.0,
                        &WriteOptions::new("api")
                    ),
                    Err(WriteError::UnsafePath(_))
                ),
                "{path:?}"
            );
        }
    }

    #[test]
    fn a_run_refused_by_an_enclosing_lock_creates_nothing() {
        let dir = TempDir::new("enclosing-first");
        write(
            &generation(&[("top.rs", "t")]),
            &dir.0,
            &WriteOptions::new("outer"),
        )
        .unwrap();
        let held = DirLock::acquire(&dir.0).unwrap();
        let nested = dir.0.join("sub/out");
        assert!(matches!(
            write(
                &generation(&[("a.rs", "a")]),
                &nested,
                &WriteOptions::new("n")
            ),
            Err(WriteError::Locked(_))
        ));
        assert!(!dir.0.join("sub").exists());
        drop(held);
    }

    #[test]
    fn a_corrupt_enclosing_manifest_is_an_error() {
        let dir = TempDir::new("corrupt-enclosing");
        fs::write(dir.0.join(".roas-codegen-outer-rust.json"), "<<<<<<< HEAD").unwrap();
        assert!(matches!(
            write(
                &generation(&[("a.rs", "a")]),
                &dir.0.join("inner"),
                &WriteOptions::new("inner")
            ),
            Err(WriteError::Manifest { .. })
        ));
    }

    #[test]
    fn names_windows_resolves_elsewhere_are_refused() {
        let dir = TempDir::new("windows-names");
        for path in [
            "PETSTO~1.RS",
            "a.rs:x",
            "aux.rs",
            "sub/CON",
            "com1.rs",
            "a?b.rs",
            "a|b.rs",
            "a\tb.rs",
            "CONOUT$.rs",
        ] {
            assert!(
                matches!(
                    write(
                        &generation(&[(path, "x")]),
                        &dir.0,
                        &WriteOptions::new("api")
                    ),
                    Err(WriteError::UnsafePath(_))
                ),
                "{path:?}"
            );
        }
        write(
            &generation(&[("com0.rs", "x"), ("auxiliary.rs", "y")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
    }

    #[test]
    fn a_manifest_in_another_case_that_is_not_ours_is_another_owner() {
        let dir = TempDir::new("manifest-variant");
        write(
            &generation(&[("a.rs", "a")]),
            &dir.0,
            &WriteOptions::new("pets"),
        )
        .unwrap();
        let ours = dir.0.join(".roas-codegen-pets-rust.json");
        let variant = dir.0.join(".roas-codegen-Pets-rust.json");
        fs::rename(&ours, &variant).unwrap();
        if ours.exists() {
            return; // case-insensitive: the variant is ours
        }
        assert!(matches!(
            write(
                &generation(&[("a.rs", "a")]),
                &dir.0,
                &WriteOptions::new("pets")
            ),
            Err(WriteError::OwnedByOther { .. })
        ));
    }

    #[test]
    fn a_nested_root_spelled_in_another_case_still_owns_its_files() {
        let dir = TempDir::new("nested-case");
        write(
            &generation(&[("types.rs", "t")]),
            &dir.0.join("Sub"),
            &WriteOptions::new("inner"),
        )
        .unwrap();
        let outer = generation(&[("sub/types.rs", "t")]);
        assert!(matches!(
            write(&outer, &dir.0, &WriteOptions::new("outer")),
            Err(WriteError::OwnedByOther { .. })
        ));
        assert!(matches!(
            check(&outer, &dir.0, &WriteOptions::new("outer")),
            Err(WriteError::OwnedByOther { .. })
        ));
    }

    #[test]
    fn a_nested_root_reached_through_unicode_folding_still_owns_its_files() {
        let dir = TempDir::new("nested-kelvin");
        let kelvin = dir.0.join("\u{212A}");
        write(
            &generation(&[("types.rs", "t")]),
            &kelvin,
            &WriteOptions::new("inner"),
        )
        .unwrap();
        if !same_file(&kelvin, &dir.0.join("K")) {
            return; // the disk keeps them apart: two directories
        }
        let outer = generation(&[("K/types.rs", "t")]);
        assert!(matches!(
            write(&outer, &dir.0, &WriteOptions::new("outer")),
            Err(WriteError::OwnedByOther { .. })
        ));
    }

    #[test]
    fn temporaries_of_files_no_longer_generated_are_removed() {
        let dir = TempDir::new("stale-temps");
        write(
            &generation(&[("pets.rs", "p")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        let leftover = dir.0.join(format!("{}1-2-3", tmp_prefix("pets.rs")));
        fs::write(&leftover, "partial").unwrap();
        write(
            &generation(&[("types.rs", "t")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        assert!(!leftover.exists());
    }

    #[cfg(unix)]
    #[test]
    fn links_before_a_dot_dot_through_a_missing_directory_are_followed() {
        let dir = TempDir::new("resolved-link");
        fs::create_dir_all(dir.0.join("real/deep")).unwrap();
        std::os::unix::fs::symlink(dir.0.join("real/deep"), dir.0.join("link")).unwrap();
        let through = resolved(&dir.0.join("link/missing/../../gen")).unwrap();
        let direct = resolved(&dir.0.join("real/gen")).unwrap();
        assert_eq!(through, direct);
    }

    #[test]
    fn an_empty_source_id_keeps_the_recorded_one() {
        let dir = TempDir::new("empty-source");
        let mut a = WriteOptions::new("api");
        a.source_id = Some("spec/a".into());
        write(&generation(&[("a.rs", "a")]), &dir.0, &a).unwrap();
        let mut empty = WriteOptions::new("api");
        empty.source_id = Some(String::new());
        write(&generation(&[("a.rs", "a")]), &dir.0, &empty).unwrap();
        let mut b = WriteOptions::new("api");
        b.source_id = Some("spec/b".into());
        assert!(matches!(
            write(&generation(&[("a.rs", "b")]), &dir.0, &b),
            Err(WriteError::SourceMismatch { .. })
        ));
    }

    #[test]
    fn a_partial_write_after_a_case_change_settles_in_one_run() {
        use crate::diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity};
        let dir = TempDir::new("partial-case");
        write(
            &generation(&[("a.rs", "a")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        fs::remove_file(dir.0.join("a.rs")).unwrap();
        let mut g = generation(&[("A.rs", "a")]);
        g.diagnostics.push(Diagnostic::new(
            Severity::Error,
            DiagnosticKind::ReferenceCycle,
            SchemaId::new(url::Url::parse("file:///x.json").unwrap(), ""),
            "",
            "test",
        ));
        let mut partial = WriteOptions::new("api");
        partial.allow_partial = true;
        write(&g, &dir.0, &partial).unwrap();
        assert!(check(&g, &dir.0, &partial).unwrap().is_clean());
    }

    #[cfg(unix)]
    #[test]
    fn the_enclosing_scan_stops_at_a_sticky_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("sticky");
        let shared = dir.0.join("shared");
        fs::create_dir_all(shared.join("mine")).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
        // Another user's junk in the shared directory blocks nothing.
        fs::write(shared.join(".roas-codegen-x-rust.json"), "not json").unwrap();
        write(
            &generation(&[("a.rs", "a")]),
            &shared.join("mine"),
            &WriteOptions::new("api"),
        )
        .unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn a_new_output_path_ending_in_dot_is_created() {
        let dir = TempDir::new("trailing-dot-out");
        write(
            &generation(&[("a.rs", "a")]),
            &dir.0.join("fresh/."),
            &WriteOptions::new("api"),
        )
        .unwrap();
        assert!(dir.0.join("fresh/a.rs").is_file());
    }

    #[test]
    fn a_file_renamed_to_a_unicode_folded_spelling_is_respelled() {
        let dir = TempDir::new("kelvin-file");
        let g = generation(&[("k.rs", "k")]);
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        fs::rename(dir.0.join("k.rs"), dir.0.join("\u{212A}.rs")).unwrap();
        if !same_file(&dir.0.join("\u{212A}.rs"), &dir.0.join("k.rs")) {
            return; // the disk keeps them apart
        }
        assert!(
            !check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        let names: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".rs"))
            .collect();
        assert_eq!(names, vec!["k.rs".to_owned()]);
    }

    #[cfg(unix)]
    #[test]
    fn a_rewritten_file_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("keep-mode");
        write(
            &generation(&[("a.rs", "1")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        fs::set_permissions(dir.0.join("a.rs"), fs::Permissions::from_mode(0o444)).unwrap();
        write(
            &generation(&[("a.rs", "2")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        let mode = fs::metadata(dir.0.join("a.rs"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        fs::set_permissions(dir.0.join("a.rs"), fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(mode, 0o444);
    }

    #[test]
    fn a_path_through_a_missing_directory_checks_clean_without_it() {
        let dir = TempDir::new("on-the-way");
        let out = dir.0.join("scratch/../gen");
        let g = generation(&[("a.rs", "a")]);
        write(&g, &out, &WriteOptions::new("api")).unwrap();
        assert!(dir.0.join("gen/a.rs").is_file());
        assert!(!dir.0.join("scratch").exists(), "nothing made on the way");
        assert!(
            check(&g, &out, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn check_waits_for_no_running_write() {
        let dir = TempDir::new("check-locked");
        let g = generation(&[("a.rs", "a")]);
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        let running = DirLock::acquire(&dir.0).unwrap();
        assert!(matches!(
            check(&g, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::Locked(_))
        ));
        drop(running);
        assert!(
            check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_misspelled_file_is_never_taken_for_it() {
        let dir = TempDir::new("misspelled-link");
        fs::write(dir.0.join("Types.rs"), "same").unwrap();
        if !same_file(&dir.0.join("Types.rs"), &dir.0.join("types.rs")) {
            return;
        }
        std::os::unix::fs::symlink("Types.rs", dir.0.join("zlink")).unwrap();
        write(
            &generation(&[("types.rs", "same")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        assert!(
            fs::symlink_metadata(dir.0.join("zlink"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_forced_write_over_a_linked_old_spelling_settles_in_one_run() {
        let dir = TempDir::new("linked-force");
        fs::write(dir.0.join("A.rs"), "hand").unwrap();
        if !same_file(&dir.0.join("A.rs"), &dir.0.join("a.rs")) {
            return;
        }
        fs::hard_link(dir.0.join("A.rs"), dir.0.join("backup")).unwrap();
        let g = generation(&[("a.rs", "generated")]);
        let mut forced = WriteOptions::new("api");
        forced.overwrite_unowned = true;
        write(&g, &dir.0, &forced).unwrap();
        assert!(check(&g, &dir.0, &forced).unwrap().is_clean());
        assert_eq!(fs::read_to_string(dir.0.join("backup")).unwrap(), "hand");
    }

    #[test]
    fn a_lock_file_that_appears_during_check_is_judged_by_where_it_is() {
        let dir = TempDir::new("appeared");
        let out = dir.0.join("out");
        let root = dir.0.clone();
        fs::create_dir_all(&out).unwrap();
        let unlocked = vec![root.clone(), out.clone()];
        assert_eq!(appeared(&out, &unlocked, false), None);
        // Lock files made for shared locks only: no write here.
        fs::write(root.join(LOCK_FILE), "").unwrap();
        fs::write(out.join(LOCK_FILE), "").unwrap();
        assert_eq!(appeared(&out, &unlocked, false), None);
        // Held exclusively: a write is running there.
        let held = DirLock::acquire(&root).unwrap();
        assert_eq!(appeared(&out, &unlocked, false), Some(root.join(LOCK_FILE)));
        drop(held);
        // In the output directory, a changed manifest: a write ran and ended.
        assert_eq!(appeared(&out, &unlocked, true), Some(out.join(LOCK_FILE)));
    }

    #[test]
    fn a_directory_stored_in_another_case_is_refused() {
        let dir = TempDir::new("dir-spelling");
        write(
            &generation(&[("Dir/a.rs", "a")]),
            &dir.0,
            &WriteOptions::new("api"),
        )
        .unwrap();
        if !same_file(&dir.0.join("Dir"), &dir.0.join("dir")) {
            return; // two directories here
        }
        let g = generation(&[("dir/a.rs", "a")]);
        assert!(matches!(
            check(&g, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::DirectorySpelling { .. })
        ));
        assert!(matches!(
            write(&g, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::DirectorySpelling { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn an_identical_linked_misspelling_gets_the_planned_name() {
        let dir = TempDir::new("linked-identical");
        fs::write(dir.0.join("A.rs"), "same").unwrap();
        if !same_file(&dir.0.join("A.rs"), &dir.0.join("a.rs")) {
            return;
        }
        fs::hard_link(dir.0.join("A.rs"), dir.0.join("backup")).unwrap();
        let g = generation(&[("a.rs", "same")]);
        assert!(
            !check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        assert!(
            check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
        let names: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".rs"))
            .collect();
        assert_eq!(names, vec!["a.rs".to_owned()]);
    }

    #[cfg(unix)]
    #[test]
    fn check_refuses_a_fifo_manifest_without_waiting() {
        let dir = TempDir::new("fifo-manifest");
        let manifest = dir.0.join(".roas-codegen-api-rust.json");
        let status = std::process::Command::new("mkfifo").arg(&manifest).status();
        if !status.is_ok_and(|s| s.success()) {
            return; // no mkfifo here
        }
        let g = generation(&[("a.rs", "a")]);
        assert!(matches!(
            check(&g, &dir.0, &WriteOptions::new("api")),
            Err(WriteError::Manifest { .. })
        ));
    }

    #[test]
    fn a_manifest_stored_in_another_case_gets_its_own_name() {
        let dir = TempDir::new("manifest-respell");
        let g = generation(&[("a.rs", "a")]);
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        let ours = dir.0.join(".roas-codegen-api-rust.json");
        let shouty = dir.0.join(".ROAS-CODEGEN-API-RUST.JSON");
        fs::rename(&ours, &shouty).unwrap();
        if !same_file(&shouty, &ours) {
            return; // case-sensitive: another target's manifest now
        }
        assert!(
            check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .manifest_outdated
        );
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        assert!(
            check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
        let names: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json") || n.ends_with(".JSON"))
            .collect();
        assert_eq!(names, vec![".roas-codegen-api-rust.json".to_owned()]);
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_manifest_in_another_case_settles_too() {
        let dir = TempDir::new("manifest-linked");
        let g = generation(&[("a.rs", "a")]);
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        let ours = dir.0.join(".roas-codegen-api-rust.json");
        let shouty = dir.0.join(".ROAS-CODEGEN-API-RUST.JSON");
        fs::rename(&ours, &shouty).unwrap();
        if !same_file(&shouty, &ours) {
            return;
        }
        fs::hard_link(&shouty, dir.0.join("backup.json")).unwrap();
        write(&g, &dir.0, &WriteOptions::new("api")).unwrap();
        assert!(
            check(&g, &dir.0, &WriteOptions::new("api"))
                .unwrap()
                .is_clean()
        );
    }
}
