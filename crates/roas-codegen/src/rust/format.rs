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

/// Format with `rustfmt` if it can be run; otherwise return the text as
/// it is. Best effort by design: syntax was already validated.
pub(crate) fn format(text: &str) -> String {
    let Ok(mut child) = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return text.to_owned();
    };
    if let Some(mut stdin) = child.stdin.take()
        && stdin.write_all(text.as_bytes()).is_err()
    {
        return text.to_owned();
    }
    match child.wait_with_output() {
        Ok(output) if output.status.success() => {
            String::from_utf8(output.stdout).unwrap_or_else(|_| text.to_owned())
        }
        _ => text.to_owned(),
    }
}
