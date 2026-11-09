//! `gofmt`, which both checks and formats: Go has no parser this crate
//! could run instead, so generating Go without the toolchain is refused
//! rather than returning unchecked text. Driven over stdin and stdout;
//! the library owns no files.

use crate::generate::GenerateError;
use std::io::Write;
use std::process::{Command, Stdio};

pub(crate) fn format(text: &str, path: &str) -> Result<String, GenerateError> {
    let mut child = Command::new("gofmt")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GenerateError::Toolchain(format!("gofmt could not be started: {e}")))?;
    // gofmt may exit before reading everything; it is always waited on,
    // so a long-running caller collects no zombies.
    let written = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
    let output = child
        .wait_with_output()
        .map_err(|e| GenerateError::Toolchain(format!("gofmt could not be waited on: {e}")))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        // gofmt names its input `<standard input>`; the path is ours.
        let reason = said
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| l.replace("<standard input>", path))
            .unwrap_or_else(|| format!("gofmt failed ({})", output.status));
        return Err(GenerateError::InvalidOutput {
            path: path.to_owned(),
            reason,
        });
    }
    if !written {
        return Err(GenerateError::Toolchain(
            "gofmt stopped reading its input".into(),
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| GenerateError::Toolchain("gofmt's output is not UTF-8".into()))
}

#[cfg(test)]
mod tests {
    use crate::generate::GenerateError;

    #[test]
    fn gofmt_formats_and_refuses_bad_syntax() {
        let formatted = match super::format("package a\nvar  x =  1\n", "a.go") {
            Ok(formatted) => formatted,
            Err(GenerateError::Toolchain(_)) => return, // no gofmt here
            Err(other) => panic!("{other}"),
        };
        assert_eq!(formatted, "package a\n\nvar x = 1\n");
        let error = super::format("package a\nfunc {\n", "a.go").unwrap_err();
        let GenerateError::InvalidOutput { path, reason } = error else {
            panic!("{error}")
        };
        assert_eq!(path, "a.go");
        assert!(reason.starts_with("a.go:"), "{reason}");
    }
}
