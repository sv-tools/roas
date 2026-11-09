//! The Go backend: allocates names, spells types, builds the view model,
//! renders, and formats with `gofmt`, which is also the syntax check.

mod format;
pub(crate) mod naming;
mod render;
mod view;

pub use view::GoDependency;

use crate::config::Config;
use crate::diagnostic::Diagnostic;
use crate::generate::{GenerateError, GeneratedFile};
use crate::ir::Ir;

pub(crate) struct Output {
    pub files: Vec<GeneratedFile>,
    pub dependencies: Vec<GoDependency>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The string literals of Go source text, decoded — interpreted or raw —
/// with comments skipped, so only real tokens are taken for import paths.
fn string_literals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                chars.by_ref().find(|&c| c == '\n');
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut star = false;
                for c in chars.by_ref() {
                    if star && c == '/' {
                        break;
                    }
                    star = c == '*';
                }
            }
            '`' => out.push(
                chars
                    .by_ref()
                    .take_while(|&c| c != '`')
                    .filter(|&c| c != '\r')
                    .collect(),
            ),
            '"' => {
                let mut literal = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => literal.extend(unescape(&mut chars)),
                        c => literal.push(c),
                    }
                }
                out.push(literal);
            }
            _ => {}
        }
    }
    out
}

/// One escape of a Go interpreted string, after its backslash.
fn unescape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<char> {
    fn number(chars: &mut impl Iterator<Item = char>, radix: u32, digits: usize) -> Option<char> {
        let text: String = chars.take(digits).collect();
        u32::from_str_radix(&text, radix)
            .ok()
            .and_then(char::from_u32)
    }
    match chars.next()? {
        'a' => Some('\u{7}'),
        'b' => Some('\u{8}'),
        'f' => Some('\u{c}'),
        'n' => Some('\n'),
        'r' => Some('\r'),
        't' => Some('\t'),
        'v' => Some('\u{b}'),
        'x' => number(chars, 16, 2),
        'u' => number(chars, 16, 4),
        'U' => number(chars, 16, 8),
        d @ '0'..='7' => {
            let rest: String = chars.by_ref().take(2).collect();
            u32::from_str_radix(&format!("{d}{rest}"), 8)
                .ok()
                .and_then(char::from_u32)
        }
        other => Some(other),
    }
}

pub(crate) fn emit(ir: &Ir, config: &Config, source: &str) -> Result<Output, GenerateError> {
    let built = view::build(ir, config, source)?;
    let rendered = render::render_file(&built.file)?;
    let path = format!("{}.go", config.go.file);
    let contents = format::format(&rendered, &path)?;
    let mut files = vec![GeneratedFile::new(path, contents)];
    if let Some(module) = &config.go.module_path {
        // Every import must resolve inside this module or the standard
        // library: anything else needs a `require` line, which needs a
        // version no substitution names. An `extra_imports` entry may hold
        // several import specs, so every string in it is a path to check.
        let extra: Vec<String> = config
            .extra_imports
            .iter()
            .flat_map(|spec| string_literals(spec))
            .collect();
        let generated = built.file.imports.iter().map(|spec| spec.trim_matches('"'));
        let paths: Vec<&str> = generated.chain(extra.iter().map(String::as_str)).collect();
        // The generated file is the module's root package: importing the
        // module itself is a cycle.
        if paths.iter().any(|path| path == module) {
            return Err(GenerateError::Config(format!(
                "an import of {module:?} is the generated package importing itself, which Go refuses as an import cycle"
            )));
        }
        let inside = |path: &str| path.starts_with(&format!("{module}/"));
        if let Some(import) = paths
            .iter()
            // `C` is cgo's pseudo-package, no module at all.
            .find(|path| **path != "C" && !naming::is_std_import(path) && !inside(path))
        {
            return Err(GenerateError::Config(format!(
                "`go.module_path` generates a go.mod, which cannot require {import:?} without a version; leave it unset and declare the module in a go.mod of your own"
            )));
        }
        files.push(GeneratedFile::new(
            "go.mod",
            format!("// Code generated by roas. DO NOT EDIT.\n\nmodule {module}\n\ngo 1.27\n"),
        ));
    }
    Ok(Output {
        files,
        dependencies: built.dependencies,
        diagnostics: built.diagnostics,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn import_strings_are_real_tokens() {
        assert_eq!(
            super::string_literals(r#"_ "time" // registers "timer""#),
            ["time"]
        );
        assert_eq!(
            super::string_literals("/* `fmt/ */ _ \"github.com/x/y\" /* ` */"),
            ["github.com/x/y"]
        );
        assert_eq!(super::string_literals(r#""\u0074ime""#), ["time"]);
        assert_eq!(super::string_literals(r#""\x74\151me""#), ["time"]);
        assert_eq!(super::string_literals("_ `a/b`; x \"c\""), ["a/b", "c"]);
    }
}
