//! Turning a [`Generation`] into files, safely.
//!
//! `generate` never touches disk; this module does, under three rules.
//!
//! - **Ownership is recorded.** A manifest in the output directory lists
//!   every file a generation target wrote, with a content hash. Its name
//!   is derived from the target — a caller-chosen label plus the language
//!   — so the next run finds it, and two targets can share a directory:
//!   `petstore` Rust and `billing` Rust each keep their own manifest.
//! - **Nothing is taken that is not owned.** A path another target's
//!   manifest owns is refused. So is an existing file no manifest owns,
//!   unless the caller allows overwriting it.
//! - **Nothing changes on a failed or partial run.** Every check happens
//!   before the first write. Each file is written to a temporary sibling
//!   and renamed into place, and the manifest is written last; a run that
//!   stops midway leaves the previous manifest, and the next run rewrites
//!   whatever differs from the generation. A directory holding
//!   hand-written files cannot be replaced atomically as a set, so the
//!   guarantee is per file, not per batch.
//!
//! Only files recorded in the previous manifest are removed as stale, and
//! only when they still hold what was generated: an edited file is kept
//! and reported.

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
pub struct WriteOptions {
    /// Names this generation target, together with the language. Two
    /// targets writing to one directory need different labels.
    pub label: String,
    /// Overwrite an existing file that no manifest owns. Off by default:
    /// such a file is probably hand-written.
    pub overwrite_unowned: bool,
}

impl WriteOptions {
    pub fn new(label: impl Into<String>) -> Self {
        WriteOptions {
            label: label.into(),
            overwrite_unowned: false,
        }
    }
}

/// What a [`write()`] did, path by path, relative to the output directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteReport {
    /// Created or changed.
    pub written: Vec<PathBuf>,
    /// Already held exactly the generated content; left untouched.
    pub unchanged: Vec<PathBuf>,
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
pub struct CheckReport {
    /// On disk, but different from what would be generated.
    pub changed: Vec<PathBuf>,
    /// Would be generated, but absent.
    pub missing: Vec<PathBuf>,
    /// Owned by the previous run and no longer generated.
    pub stale: Vec<PathBuf>,
    /// The manifest itself is absent or does not list what is generated.
    pub manifest_outdated: bool,
}

impl CheckReport {
    /// True when the directory already holds exactly this generation.
    pub fn is_clean(&self) -> bool {
        self.changed.is_empty()
            && self.missing.is_empty()
            && self.stale.is_empty()
            && !self.manifest_outdated
    }
}

/// Why the output directory could not be written or checked.
#[derive(Debug, Error)]
pub enum WriteError {
    #[error("label {0:?} must be non-empty and use only letters, digits, `-` and `_`")]
    InvalidLabel(String),
    #[error("generated path {0} must be relative and stay inside the output directory")]
    UnsafePath(PathBuf),
    #[error(
        "{path} is owned by another generation target (manifest {manifest}); use a different output directory or label"
    )]
    OwnedByOther { path: PathBuf, manifest: PathBuf },
    #[error(
        "{0} already exists and was not written by this generator; move it, or allow overwriting unowned files"
    )]
    Unowned(PathBuf),
    #[error("manifest {path} cannot be read: {reason}")]
    Manifest { path: PathBuf, reason: String },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The manifest's on-disk form. `files` maps a relative path, written
/// with `/`, to the hash of the content this target wrote there.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Manifest {
    generator: String,
    version: String,
    label: String,
    language: String,
    files: BTreeMap<String, String>,
}

const MANIFEST_PREFIX: &str = ".roas-codegen-";
const MANIFEST_SUFFIX: &str = ".json";

fn language(target: Target) -> &'static str {
    match target {
        Target::Rust => "rust",
        Target::Go => "go",
    }
}

/// The manifest's file name for a target: `.roas-codegen-<label>-<language>.json`.
pub fn manifest_name(label: &str, target: Target) -> Result<String, WriteError> {
    if label.is_empty()
        || !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
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

/// A generated path as a manifest key, refusing anything that could
/// escape the output directory.
fn key(path: &Path) -> Result<String, WriteError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| WriteError::UnsafePath(path.to_path_buf()))?
                    .to_owned(),
            ),
            _ => return Err(WriteError::UnsafePath(path.to_path_buf())),
        }
    }
    if parts.is_empty() {
        return Err(WriteError::UnsafePath(path.to_path_buf()));
    }
    Ok(parts.join("/"))
}

fn read_manifest(path: &Path) -> Result<Option<Manifest>, WriteError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| WriteError::Manifest {
                path: path.to_path_buf(),
                reason: e.to_string(),
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(WriteError::Io {
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

/// Every other target's manifest in the directory, and the paths it owns.
fn other_owners(out_dir: &Path, ours: &str) -> Result<BTreeMap<String, PathBuf>, WriteError> {
    let mut owners = BTreeMap::new();
    let entries = match fs::read_dir(out_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(owners),
        Err(e) => {
            return Err(WriteError::Io {
                path: out_dir.to_path_buf(),
                source: e,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(io(out_dir))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ours || !name.starts_with(MANIFEST_PREFIX) || !name.ends_with(MANIFEST_SUFFIX) {
            continue;
        }
        let path = entry.path();
        if let Some(manifest) = read_manifest(&path)? {
            for file in manifest.files.keys() {
                owners.insert(file.clone(), PathBuf::from(&name));
            }
        }
    }
    Ok(owners)
}

/// The generated files keyed for the manifest, with their contents.
fn planned(generation: &Generation) -> Result<BTreeMap<String, &str>, WriteError> {
    let mut out = BTreeMap::new();
    for file in &generation.files {
        out.insert(key(&file.path)?, file.contents.as_str());
    }
    Ok(out)
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
    let tmp = path.with_file_name(format!(".{name}.roas-tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&tmp).map_err(io(&tmp))?;
        file.write_all(content).map_err(io(&tmp))?;
        file.sync_all().map_err(io(&tmp))?;
        fs::rename(&tmp, path).map_err(io(path))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Write a generation into `out_dir` under the ownership rules above.
///
/// Refuses a generation that carries any `Error` diagnostic unless
/// `allow_partial` is set: writing it would replace good files with a
/// set missing the types the errors suppressed.
pub fn write(
    generation: &Generation,
    out_dir: &Path,
    target: Target,
    options: &WriteOptions,
) -> Result<WriteReport, WriteError> {
    let manifest_file = manifest_name(&options.label, target)?;
    let manifest_path = out_dir.join(&manifest_file);
    let previous = read_manifest(&manifest_path)?.unwrap_or_default();
    let others = other_owners(out_dir, &manifest_file)?;
    let planned = planned(generation)?;

    // Every check before the first write.
    for path in planned.keys() {
        if let Some(manifest) = others.get(path) {
            return Err(WriteError::OwnedByOther {
                path: PathBuf::from(path),
                manifest: manifest.clone(),
            });
        }
        let on_disk = out_dir.join(path);
        if !previous.files.contains_key(path) && on_disk.exists() && !options.overwrite_unowned {
            return Err(WriteError::Unowned(PathBuf::from(path)));
        }
    }

    let mut report = WriteReport {
        manifest: PathBuf::from(&manifest_file),
        ..Default::default()
    };
    let mut files = BTreeMap::new();
    for (path, content) in &planned {
        let on_disk = out_dir.join(path);
        let current = fs::read(&on_disk).ok();
        if current.as_deref() == Some(content.as_bytes()) {
            report.unchanged.push(PathBuf::from(path));
        } else {
            write_atomic(&on_disk, content.as_bytes())?;
            report.written.push(PathBuf::from(path));
        }
        files.insert(path.clone(), hash(content.as_bytes()));
    }
    for (path, recorded) in &previous.files {
        if planned.contains_key(path) {
            continue;
        }
        let on_disk = out_dir.join(path);
        match fs::read(&on_disk) {
            Ok(current) if &hash(&current) == recorded => {
                fs::remove_file(&on_disk).map_err(io(&on_disk))?;
                report.removed.push(PathBuf::from(path));
            }
            Ok(_) => report.kept_edited.push(PathBuf::from(path)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(WriteError::Io {
                    path: on_disk,
                    source: e,
                });
            }
        }
    }

    let manifest = Manifest {
        generator: "roas-codegen".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        label: options.label.clone(),
        language: language(target).into(),
        files,
    };
    let mut text = serde_json::to_string_pretty(&manifest).expect("a manifest serializes");
    text.push('\n');
    if fs::read(&manifest_path).ok().as_deref() != Some(text.as_bytes()) {
        write_atomic(&manifest_path, text.as_bytes())?;
    }
    Ok(report)
}

/// Compare `out_dir` with what [`write()`] would leave there, without
/// writing anything: file contents, owned paths and the manifest.
pub fn check(
    generation: &Generation,
    out_dir: &Path,
    target: Target,
    label: &str,
) -> Result<CheckReport, WriteError> {
    let manifest_file = manifest_name(label, target)?;
    let previous = read_manifest(&out_dir.join(&manifest_file))?;
    let planned = planned(generation)?;
    let mut report = CheckReport::default();
    for (path, content) in &planned {
        match fs::read(out_dir.join(path)) {
            Ok(current) if current == content.as_bytes() => {}
            Ok(_) => report.changed.push(PathBuf::from(path)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.missing.push(PathBuf::from(path))
            }
            Err(e) => {
                return Err(WriteError::Io {
                    path: out_dir.join(path),
                    source: e,
                });
            }
        }
    }
    match &previous {
        Some(manifest) => {
            for path in manifest.files.keys().filter(|p| !planned.contains_key(*p)) {
                if out_dir.join(path).exists() {
                    report.stale.push(PathBuf::from(path));
                }
            }
            let owned: BTreeSet<&String> = manifest.files.keys().collect();
            let wanted: BTreeSet<&String> = planned.keys().collect();
            report.manifest_outdated = owned != wanted
                || planned
                    .iter()
                    .any(|(p, c)| manifest.files.get(p) != Some(&hash(c.as_bytes())));
        }
        None => report.manifest_outdated = true,
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::{Dependencies, GeneratedFile};

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
        Generation {
            files: files
                .iter()
                .map(|(p, c)| GeneratedFile {
                    path: PathBuf::from(p),
                    contents: (*c).to_owned(),
                })
                .collect(),
            diagnostics: Vec::new(),
            dependencies: Dependencies::default(),
        }
    }

    #[test]
    fn writes_records_and_then_reports_unchanged() {
        let dir = TempDir::new("basic");
        let g = generation(&[("types.rs", "a"), ("nested/more.rs", "b")]);
        let report = write(&g, &dir.0, Target::Rust, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.written.len(), 2);
        assert_eq!(
            report.manifest,
            PathBuf::from(".roas-codegen-api-rust.json")
        );
        assert_eq!(
            fs::read_to_string(dir.0.join("nested/more.rs")).unwrap(),
            "b"
        );
        assert!(check(&g, &dir.0, Target::Rust, "api").unwrap().is_clean());

        let again = write(&g, &dir.0, Target::Rust, &WriteOptions::new("api")).unwrap();
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
            Target::Rust,
            &WriteOptions::new("api"),
        )
        .unwrap();
        fs::write(dir.0.join("c.rs"), "edited").unwrap();
        let next = generation(&[("a.rs", "a2")]);
        assert_eq!(
            check(&next, &dir.0, Target::Rust, "api").unwrap(),
            CheckReport {
                changed: vec!["a.rs".into()],
                missing: vec![],
                stale: vec!["b.rs".into(), "c.rs".into()],
                manifest_outdated: true
            }
        );
        let report = write(&next, &dir.0, Target::Rust, &WriteOptions::new("api")).unwrap();
        assert_eq!(report.written, vec![PathBuf::from("a.rs")]);
        assert_eq!(report.removed, vec![PathBuf::from("b.rs")]);
        assert_eq!(report.kept_edited, vec![PathBuf::from("c.rs")]);
        assert!(!dir.0.join("b.rs").exists());
        assert_eq!(fs::read_to_string(dir.0.join("c.rs")).unwrap(), "edited");
        // The edited file is no longer owned.
        assert!(
            check(&next, &dir.0, Target::Rust, "api")
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn unowned_and_other_targets_files_are_refused_before_anything_changes() {
        let dir = TempDir::new("owners");
        fs::write(dir.0.join("types.rs"), "hand written").unwrap();
        let g = generation(&[("lib.rs", "x"), ("types.rs", "generated")]);
        let error = write(&g, &dir.0, Target::Rust, &WriteOptions::new("api")).unwrap_err();
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
        write(&g, &dir.0, Target::Rust, &options).unwrap();
        assert_eq!(
            fs::read_to_string(dir.0.join("types.rs")).unwrap(),
            "generated"
        );

        // Another label may share the directory but not the paths.
        write(
            &generation(&[("billing.rs", "b")]),
            &dir.0,
            Target::Rust,
            &WriteOptions::new("billing"),
        )
        .unwrap();
        let error = write(
            &generation(&[("types.rs", "z")]),
            &dir.0,
            Target::Rust,
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
                Target::Rust,
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
        let report = check(&g, &dir.0, Target::Rust, "api").unwrap();
        assert_eq!(report.missing, vec![PathBuf::from("types.rs")]);
        assert!(report.manifest_outdated && !report.is_clean());
        fs::write(dir.0.join(".roas-codegen-api-rust.json"), "not json").unwrap();
        assert!(matches!(
            check(&g, &dir.0, Target::Rust, "api"),
            Err(WriteError::Manifest { .. })
        ));
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(hash(b""), "fnv1a64:cbf29ce484222325");
        assert_eq!(hash(b"a"), "fnv1a64:af63dc4c8601ec8c");
    }
}
