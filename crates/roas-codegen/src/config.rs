//! What a configuration file may say, and what generation consumes.
//!
//! Two types, because "what the file may omit" and "what generation
//! needs" are different sets. [`ConfigFile`] is the serde type:
//! everything optional, unknown keys rejected, relative paths resolved
//! against the file's own directory. [`Config`] is fully resolved, with
//! no `Option` where the generator needs an answer, and
//! [`ConfigFile::build`] is the one place a missing `target` is refused.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// The language to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Rust,
    Go,
}

/// How generated types are spread over files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// One file per module or package. The default, and the only layout
    /// under which a "footer after all generated code" means what it
    /// says.
    #[default]
    OneFilePerModule,
    /// One file per generated type.
    FilePerType,
}

/// How a substituted type is spelled in one language.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct TypeSpelling {
    /// The type as it appears in generated code, such as
    /// `chrono::DateTime<chrono::Utc>` or `time.Time`.
    pub name: String,
    /// The import that makes it available: a crate for Rust, an import
    /// path for Go.
    pub import: Option<String>,
}

/// A user-provided type standing in for a schema or a `format`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Substitution {
    pub rust: Option<TypeSpelling>,
    pub go: Option<TypeSpelling>,
}

/// Per-field settings, addressed by the property name as written in
/// the description.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct FieldConfig {
    /// Extra Rust attributes, appended as separate lines.
    pub attrs: Vec<String>,
    /// Extra Go struct-tag keys, merged into the one tag.
    pub go_tags: BTreeMap<String, String>,
    /// The generated name, when the derived one is not wanted.
    pub rename: Option<String>,
}

/// Per-type settings, addressed by the component name as written in
/// the description.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct TypeConfig {
    /// Extra Rust container attributes.
    pub attrs: Vec<String>,
    /// The generated name, when the derived one is not wanted.
    pub rename: Option<String>,
    /// Opt this type out of generated validation, once validation is on.
    pub validation: Option<bool>,
    pub fields: BTreeMap<String, FieldConfig>,
}

/// Rust settings as a file may state them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct RustConfigFile {
    pub crate_name: Option<String>,
    pub edition: Option<String>,
    /// Derives added to every generated container.
    pub derives: Vec<String>,
}

/// Go settings as a file may state them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct GoConfigFile {
    pub package: Option<String>,
    pub module_path: Option<String>,
}

/// What a configuration file may say. Everything optional; nothing
/// validated beyond the shape.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(default, deny_unknown_fields, rename_all = "snake_case")]
pub struct ConfigFile {
    pub target: Option<Target>,
    pub layout: Option<Layout>,
    pub validation: Option<bool>,
    pub exact_numbers: Option<bool>,
    pub header: Option<String>,
    pub extra_imports: Vec<String>,
    pub allow_codegen_extensions: Option<bool>,
    /// A directory of templates overriding the built-in ones. Relative
    /// paths resolve against the configuration file's directory.
    pub template_dir: Option<PathBuf>,
    pub rust: RustConfigFile,
    pub go: GoConfigFile,
    /// Keyed by `format` name or by component name.
    pub substitutions: BTreeMap<String, Substitution>,
    pub types: BTreeMap<String, TypeConfig>,
}

impl ConfigFile {
    /// Read and parse a file. Relative paths inside it resolve against
    /// the file's own directory, never the working directory, so a
    /// checked-in configuration means the same thing from every shell.
    pub fn from_path(path: &Path) -> Result<ConfigFile, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        ConfigFile::from_toml(&text, base).map_err(|error| match error {
            ConfigError::Parse { source, .. } => ConfigError::Parse {
                path: Some(path.to_path_buf()),
                source,
            },
            other => other,
        })
    }

    /// Parse TOML text. `base` is what relative paths resolve against;
    /// it is taken explicitly because text has no location of its own.
    pub fn from_toml(text: &str, base: &Path) -> Result<ConfigFile, ConfigError> {
        let mut file: ConfigFile =
            toml::from_str(text).map_err(|source| ConfigError::Parse { path: None, source })?;
        if let Some(dir) = file.template_dir.take() {
            file.template_dir = Some(if dir.is_absolute() {
                dir
            } else {
                base.join(dir)
            });
        }
        Ok(file)
    }

    /// Layer `over` on top of `self`: a value `over` sets wins, a value
    /// it leaves unset keeps `self`'s. Lists and maps combine, later
    /// entries winning on the same key. This is how a command line
    /// applies to a file: parse the file, build a `ConfigFile` from the
    /// flags, merge.
    pub fn merge(mut self, over: ConfigFile) -> ConfigFile {
        self.target = over.target.or(self.target);
        self.layout = over.layout.or(self.layout);
        self.validation = over.validation.or(self.validation);
        self.exact_numbers = over.exact_numbers.or(self.exact_numbers);
        self.header = over.header.or(self.header);
        self.extra_imports.extend(over.extra_imports);
        self.allow_codegen_extensions = over
            .allow_codegen_extensions
            .or(self.allow_codegen_extensions);
        self.template_dir = over.template_dir.or(self.template_dir);
        self.rust.crate_name = over.rust.crate_name.or(self.rust.crate_name);
        self.rust.edition = over.rust.edition.or(self.rust.edition);
        self.rust.derives.extend(over.rust.derives);
        self.go.package = over.go.package.or(self.go.package);
        self.go.module_path = over.go.module_path.or(self.go.module_path);
        self.substitutions.extend(over.substitutions);
        self.types.extend(over.types);
        self
    }

    /// Resolve into what generation consumes. This is where "required"
    /// is enforced: a missing `target` is an error here and nowhere else.
    pub fn build(self) -> Result<Config, ConfigError> {
        let target = self.target.ok_or(ConfigError::MissingTarget)?;
        Ok(Config {
            target,
            layout: self.layout.unwrap_or_default(),
            validation: self.validation.unwrap_or(false),
            exact_numbers: self.exact_numbers.unwrap_or(false),
            header: self.header,
            extra_imports: self.extra_imports,
            allow_codegen_extensions: self.allow_codegen_extensions.unwrap_or(false),
            template_dir: self.template_dir,
            rust: RustConfig {
                crate_name: self.rust.crate_name,
                edition: self.rust.edition.unwrap_or_else(|| "2024".to_owned()),
                derives: self.rust.derives,
            },
            go: GoConfig {
                package: self.go.package.unwrap_or_else(|| "api".to_owned()),
                module_path: self.go.module_path,
            },
            substitutions: self.substitutions,
            types: self.types,
        })
    }
}

/// Rust settings, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustConfig {
    pub crate_name: Option<String>,
    pub edition: String,
    pub derives: Vec<String>,
}

/// Go settings, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoConfig {
    pub package: String,
    pub module_path: Option<String>,
}

/// What generation consumes. Fully resolved: no `Option` where the
/// generator needs an answer, no `Deserialize`, no `Default` — obtained
/// only through [`ConfigFile::build`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub target: Target,
    pub layout: Layout,
    pub validation: bool,
    /// Cargo-gated: honoured only when this crate was built with the
    /// `exact-numbers` feature and the source has exact fidelity.
    pub exact_numbers: bool,
    pub header: Option<String>,
    pub extra_imports: Vec<String>,
    pub allow_codegen_extensions: bool,
    /// Absolute by now.
    pub template_dir: Option<PathBuf>,
    pub rust: RustConfig,
    pub go: GoConfig,
    pub substitutions: BTreeMap<String, Substitution>,
    pub types: BTreeMap<String, TypeConfig>,
}

/// Why a configuration could not be loaded or resolved.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read configuration file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("configuration{} is not valid: {source}", match path { Some(p) => format!(" file {}", p.display()), None => String::new() })]
    Parse {
        path: Option<PathBuf>,
        #[source]
        source: toml::de::Error,
    },
    #[error(
        "no target language: set `target = \"rust\"` or `target = \"go\"` in the configuration, or pass `--target`"
    )]
    MissingTarget,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_two_line_file_parses_and_builds_with_a_target() {
        let file = ConfigFile::from_toml(
            "[types.Pet.fields.tag]\nattrs = ['#[validate(length(min = 1))]']\n",
            Path::new("/cfg"),
        )
        .unwrap();
        assert_eq!(
            file.types["Pet"].fields["tag"].attrs,
            vec!["#[validate(length(min = 1))]".to_owned()]
        );
        assert!(matches!(
            file.clone().build(),
            Err(ConfigError::MissingTarget)
        ));
        let config = file
            .merge(ConfigFile {
                target: Some(Target::Rust),
                ..Default::default()
            })
            .build()
            .unwrap();
        assert_eq!(config.target, Target::Rust);
        assert_eq!(config.layout, Layout::OneFilePerModule);
        assert!(!config.validation);
        assert_eq!(config.rust.edition, "2024");
        assert_eq!(config.go.package, "api");
    }

    #[test]
    fn unknown_keys_are_rejected_at_every_level() {
        for text in [
            "validaton = true\n",
            "[rust]\ncrate = \"x\"\n",
            "[types.Pet]\nattr = []\n",
            "[types.Pet.fields.tag]\nattr = []\n",
            "[substitutions.uuid.rust]\ntype = \"x\"\n",
        ] {
            let error = ConfigFile::from_toml(text, Path::new("/cfg")).unwrap_err();
            assert!(
                matches!(error, ConfigError::Parse { .. }),
                "{text:?} -> {error}"
            );
        }
    }

    #[test]
    fn relative_template_dir_resolves_against_the_base() {
        let file = ConfigFile::from_toml("template_dir = \"templates\"\n", Path::new("/proj/cfg"))
            .unwrap();
        assert_eq!(
            file.template_dir.as_deref(),
            Some(Path::new("/proj/cfg/templates"))
        );
        let file =
            ConfigFile::from_toml("template_dir = \"/abs\"\n", Path::new("/proj/cfg")).unwrap();
        assert_eq!(file.template_dir.as_deref(), Some(Path::new("/abs")));
    }

    #[test]
    fn from_path_resolves_against_the_files_directory_and_names_it_on_error() {
        let dir = std::env::temp_dir().join(format!("roas-codegen-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("codegen.toml");
        std::fs::write(&path, "target = \"go\"\ntemplate_dir = \"tpl\"\n").unwrap();
        let file = ConfigFile::from_path(&path).unwrap();
        assert_eq!(
            file.template_dir.as_deref(),
            Some(dir.join("tpl").as_path())
        );
        std::fs::write(&path, "target = 1\n").unwrap();
        let error = ConfigFile::from_path(&path).unwrap_err();
        assert!(error.to_string().contains("codegen.toml"), "{error}");
        let missing = ConfigFile::from_path(&dir.join("absent.toml")).unwrap_err();
        assert!(matches!(missing, ConfigError::Read { .. }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn merge_lets_later_values_win_and_combines_collections() {
        let base = ConfigFile::from_toml(
            "target = \"rust\"\nvalidation = true\nextra_imports = [\"a\"]\n[rust]\nderives = [\"Eq\"]\n[types.Pet]\nrename = \"Animal\"\n",
            Path::new("/"),
        )
        .unwrap();
        let over = ConfigFile::from_toml(
            "target = \"go\"\nextra_imports = [\"b\"]\n[types.Owner]\nrename = \"Person\"\n",
            Path::new("/"),
        )
        .unwrap();
        let merged = base.merge(over);
        assert_eq!(merged.target, Some(Target::Go));
        assert_eq!(merged.validation, Some(true));
        assert_eq!(merged.extra_imports, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(merged.rust.derives, vec!["Eq".to_owned()]);
        assert_eq!(merged.types.len(), 2);
    }

    #[test]
    fn substitutions_and_layout_parse() {
        let file = ConfigFile::from_toml(
            "target = \"rust\"\nlayout = \"file_per_type\"\n[substitutions.uuid]\nrust = { name = \"uuid::Uuid\", import = \"uuid\" }\ngo = { name = \"uuid.UUID\" }\n",
            Path::new("/"),
        )
        .unwrap();
        let config = file.build().unwrap();
        assert_eq!(config.layout, Layout::FilePerType);
        let uuid = &config.substitutions["uuid"];
        assert_eq!(uuid.rust.as_ref().unwrap().import.as_deref(), Some("uuid"));
        assert_eq!(uuid.go.as_ref().unwrap().name, "uuid.UUID");
        assert!(uuid.go.as_ref().unwrap().import.is_none());
    }
}
