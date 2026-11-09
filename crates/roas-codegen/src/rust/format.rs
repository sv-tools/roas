//! Validate always; format when a formatter is there.
//!
//! `syn` parses the rendered text and the parse is thrown away — the
//! text is never regenerated from an AST, so comments, the header and
//! the generated-code marker all survive. `rustfmt`, when present, is
//! driven over stdin and stdout; the library owns no files.

use crate::generate::GenerateError;
use std::io::Write;
use std::process::{Command, Stdio};

pub(crate) fn validate(text: &str, path: &str) -> Result<(), GenerateError> {
    syn::parse_file(text)
        .map(|_| ())
        .map_err(|error| GenerateError::InvalidOutput {
            path: path.to_owned(),
            reason: error.to_string(),
        })
}

/// Format with `rustfmt`, or `None` when it cannot be run.
///
/// The output must not depend on where the generator runs, so `rustfmt`
/// never searches for a configuration: it gets an empty one explicitly,
/// which means rustfmt's defaults for the 2024 edition everywhere. The
/// `rustfmt` version still matters, as it would for any formatted file.
pub(crate) fn format(text: &str) -> Option<String> {
    let config = PrivateConfig::create()?;
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout", "--config-path"])
        .arg(config.file())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(text.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// An empty `rustfmt.toml` in a directory only this call created: the name
/// is unpredictable, the directory and the file are both created
/// exclusively, and both are removed when the formatting is done.
struct PrivateConfig(std::path::PathBuf);

impl PrivateConfig {
    fn create() -> Option<PrivateConfig> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        for _ in 0..16 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "roas-codegen-rustfmt-{}-{nanos:x}-{n}",
                std::process::id()
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => {
                    let config = PrivateConfig(dir);
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(config.file())
                        .ok()?;
                    return Some(config);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return None,
            }
        }
        None
    }

    fn file(&self) -> std::path::PathBuf {
        self.0.join("rustfmt.toml")
    }
}

impl Drop for PrivateConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
