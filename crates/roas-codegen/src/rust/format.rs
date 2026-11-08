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
    let config = empty_config()?;
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout", "--config-path"])
        .arg(&config)
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

/// An empty `rustfmt.toml` in a private directory, created once.
fn empty_config() -> Option<std::path::PathBuf> {
    static CONFIG: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let dir =
                std::env::temp_dir().join(format!("roas-codegen-rustfmt-{}", std::process::id()));
            std::fs::create_dir_all(&dir).ok()?;
            let path = dir.join("rustfmt.toml");
            std::fs::write(&path, "").ok()?;
            Some(path)
        })
        .clone()
}
