//! Tolerant reading for editors: what a damaged description still says.
//!
//! An editor holds text that is wrong most of the time it is being typed.
//! [`Document::read`] never fails: it returns an [`Outline`] of what the
//! text still says — sources, workflows and their steps, components, each
//! with its place in the text — and every [`Diagnostic`] it finds, located
//! where the source map can place it.
//!
//! - Text that is not JSON or YAML gets a located syntax error. The text up
//!   to the line of the error is read again, so a YAML description usually
//!   still has an outline of everything above that line; the outline is then
//!   marked incomplete ([`Document::is_complete`]) and the description is not
//!   checked further.
//! - Text that reads in full is checked part by part: `arazzo`, `$self`,
//!   `info`, each source description, each workflow and `components`, so a
//!   broken workflow does not hide another. When every part fits the model,
//!   the description is validated and its unknown members are listed as
//!   warnings.
//!
//! Nothing here can be run. A [`Document`] never hands out a typed
//! description, and its outline is plain strings and ranges, so no damaged
//! text reaches preparation or execution. To run a description, deserialize
//! its text the usual way, which refuses anything that does not fit.
//!
//! Positions are [`SourcePosition`]s: UTF-8 byte offsets with one-based
//! lines and columns counted in Unicode scalar values.
//! [`Document::utf16_column`] gives the column an LSP client counts.

use crate::source_map::{SourceMap, SourcePosition, SourceRange, line_of, line_starts};
use crate::unknown::UnknownMembers;
use crate::validation::{Validate, ValidationOptions};
use enumset::EnumSet;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// How much a [`Diagnostic`] matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Severity {
    /// The text is not a valid description.
    Error,
    /// Valid, but something is probably not what was meant.
    Warning,
}

/// A finding about the text.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Diagnostic {
    /// How much it matters.
    pub severity: Severity,
    /// Model-style path of the field it is about, as validation findings
    /// name fields (`#.workflows[0].steps[1].stepId`); `None` for a syntax
    /// error.
    pub path: Option<String>,
    /// Where it is in the text, when that is known: the field's value, or
    /// its name for a finding about the name, or the nearest enclosing
    /// object for a field that is missing.
    pub range: Option<SourceRange>,
    /// What is wrong.
    pub message: String,
}

/// What a description says, read leniently: a member of the wrong type is
/// simply absent here, and the diagnostics say why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Outline {
    /// `info.title`.
    pub title: Option<String>,
    /// `sourceDescriptions`, in order.
    pub sources: Vec<SourceOutline>,
    /// `workflows`, in order.
    pub workflows: Vec<WorkflowOutline>,
    /// Every named member of every `components` map.
    pub components: Vec<ComponentOutline>,
}

/// A source description.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SourceOutline {
    /// Model-style path: `#.sourceDescriptions[0]`.
    pub path: String,
    /// Where it is written.
    pub range: Option<SourceRange>,
    /// `name`.
    pub name: Option<String>,
    /// `url`.
    pub url: Option<String>,
    /// `type`, as written.
    pub kind: Option<String>,
}

/// A workflow.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkflowOutline {
    /// Model-style path: `#.workflows[0]`.
    pub path: String,
    /// Where it is written.
    pub range: Option<SourceRange>,
    /// `workflowId`.
    pub id: Option<String>,
    /// `summary`.
    pub summary: Option<String>,
    /// `steps`, in order.
    pub steps: Vec<StepOutline>,
}

/// A step.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StepOutline {
    /// Model-style path: `#.workflows[0].steps[1]`.
    pub path: String,
    /// Where it is written.
    pub range: Option<SourceRange>,
    /// `stepId`.
    pub id: Option<String>,
    /// What it calls, when one of its targets is written.
    pub target: Option<StepTarget>,
}

/// What a step calls, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StepTarget {
    /// `operationId`.
    OperationId(String),
    /// `operationPath`.
    OperationPath(String),
    /// `workflowId`.
    WorkflowId(String),
    /// `channelPath` (v1.1).
    ChannelPath(String),
}

/// A named member of a `components` map.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ComponentOutline {
    /// Model-style path: `#.components.parameters.page`.
    pub path: String,
    /// Where it is written.
    pub range: Option<SourceRange>,
    /// The map it is in: `inputs`, `parameters`, `successActions`, …
    pub kind: String,
    /// Its name.
    pub name: String,
}

/// A description read for an editor; see the [module docs](self).
#[derive(Clone, Debug)]
pub struct Document {
    text: String,
    map: Option<SourceMap>,
    version: Option<String>,
    outline: Option<Outline>,
    complete: bool,
    diagnostics: Vec<Diagnostic>,
}

impl Document {
    /// Read `text`, JSON or YAML, as far as it goes. `options` tunes
    /// validation as for [`Validate::validate`].
    ///
    /// ```
    /// # #[cfg(feature = "v1_1")] {
    /// use roas_arazzo::editor::{Document, Severity};
    ///
    /// let text = "arazzo: 1.1.0
    /// info: {title: Pets, version: '1'}
    /// sourceDescriptions: [{name: api, url: api.yaml}]
    /// workflows:
    ///   - workflowId: buy
    ///     steps:
    ///       - stepId: find
    ///         operationId: findPet
    ///   - workflowId: [oops]
    ///     steps: []
    /// ";
    /// let document = Document::read(text, Default::default());
    /// let outline = document.outline().unwrap();
    /// assert_eq!(outline.workflows[0].id.as_deref(), Some("buy"));
    /// assert_eq!(outline.workflows[0].steps[0].id.as_deref(), Some("find"));
    /// // The second workflow is still there; its id is not a string.
    /// assert_eq!(outline.workflows[1].id, None);
    /// let error = &document.diagnostics()[0];
    /// assert_eq!(error.severity, Severity::Error);
    /// assert_eq!(error.path.as_deref(), Some("#.workflows[1].workflowId"));
    /// assert_eq!(error.range.unwrap().start.line, 9);
    /// # }
    /// ```
    pub fn read(text: impl Into<String>, options: EnumSet<ValidationOptions>) -> Self {
        let text = text.into();
        let mut diagnostics = Vec::new();
        let (value, read) = match crate::source_map::parse::value(&text) {
            Ok(value) => (Some(value), text.len()),
            Err(error) => {
                let at = error_offset(&text, &error);
                let position = position_in(&text, at);
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    path: None,
                    range: Some(SourceRange {
                        start: position,
                        end: position_in(&text, next_char(&text, position.offset)),
                    }),
                    message: reason(&error),
                });
                recover(&text, position.offset)
            }
        };
        let complete = read == text.len() && value.is_some();
        let map = value
            .as_ref()
            .and_then(|_| SourceMap::parse(&text[..read]).ok());
        let version = value
            .as_ref()
            .and_then(|value| value.get("arazzo"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let outline = value
            .as_ref()
            .filter(|value| value.is_object())
            .map(|value| outline(value, map.as_ref()));
        if complete && let Some(value) = &value {
            let mut checks = Checks {
                map: map.as_ref(),
                diagnostics: &mut diagnostics,
            };
            checks.description(value, options);
        }
        Self {
            text,
            map,
            version,
            outline,
            complete,
            diagnostics,
        }
    }

    /// The text that was read.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The map of the text that was read: of all of it when it is complete,
    /// else of what came before the syntax error.
    #[must_use]
    pub fn source_map(&self) -> Option<&SourceMap> {
        self.map.as_ref()
    }

    /// The `arazzo` version the text declares, as written.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// What the text says, when enough of it is a JSON or YAML object.
    #[must_use]
    pub fn outline(&self) -> Option<&Outline> {
        self.outline.as_ref()
    }

    /// Whether the whole text was read. When it was not, the outline is
    /// that of the text before the syntax error, and the description was
    /// not checked any further.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Every finding, syntax error first, then in the order the parts were
    /// checked.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Whether any finding is an error.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    /// The one-based column of `position` counted in UTF-16 code units, as
    /// an LSP client counts them (zero-based there: subtract one).
    #[must_use]
    pub fn utf16_column(&self, position: SourcePosition) -> usize {
        crate::source_map::utf16_column(&self.text, position)
    }
}

/// How many lines above a syntax error [`recover`] tries.
const RECOVERY_LINES: usize = 16;

/// The value of the longest text that ends at a line start at or before
/// `offset` and reads, trying no more than [`RECOVERY_LINES`] of them: the
/// error is often reported a line or two after the construct that caused
/// it, such as an unterminated quote. Returns how much was read.
fn recover(text: &str, offset: usize) -> (Option<Value>, usize) {
    let starts = line_starts(text);
    // The line of the error, then each line above it.
    let mut line = starts.partition_point(|&start| start <= offset) - 1;
    for _ in 0..RECOVERY_LINES {
        let cut = starts[line];
        if cut == 0 {
            break;
        }
        if let Ok(value) = crate::source_map::parse::value(&text[..cut]) {
            return (Some(value), cut);
        }
        line -= 1;
    }
    (None, 0)
}

/// The reason of a syntax error, without the location and the snippet
/// the range already gives.
fn reason(error: &serde_saphyr::Error) -> String {
    let shown = error.to_string();
    let first = shown.lines().next().unwrap_or_default();
    let first = first.strip_prefix("error: ").unwrap_or(first);
    // `line 10 column 5: invalid indentation …`
    let reason = match first.split_once(": ") {
        Some((at, reason)) if at.starts_with("line ") && at.contains(" column ") => reason,
        _ => first,
    };
    // `unclosed bracket '[' at line 3, column 4`
    match reason.rsplit_once(" at line ") {
        Some((reason, at))
            if at.split_once(", column ").is_some_and(|(line, column)| {
                [line, column]
                    .iter()
                    .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            }) =>
        {
            reason
        }
        _ => reason,
    }
    .to_owned()
}

/// Where in `text` a syntax error is.
fn error_offset(text: &str, error: &serde_saphyr::Error) -> usize {
    let Some(location) = error.location() else {
        return text.len();
    };
    if let Some(offset) = location.span().byte_offset() {
        return usize::try_from(offset).map_or(text.len(), |offset| offset.min(text.len()));
    }
    offset_of(
        text,
        usize::try_from(location.line()).unwrap_or(usize::MAX),
        usize::try_from(location.column()).unwrap_or(usize::MAX),
    )
}

/// The byte offset of a one-based line and column, the column in
/// characters; past the end of the line is its end.
fn offset_of(text: &str, line: usize, column: usize) -> usize {
    let starts = line_starts(text);
    let Some(&start) = starts.get(line.saturating_sub(1)) else {
        return text.len();
    };
    // A byte-order mark is not a column.
    let start = if start == 0 && text.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        start
    };
    let rest = &text[start..];
    let line_end = start + rest.find(['\n', '\r']).unwrap_or(rest.len());
    rest.char_indices()
        .take_while(|&(_, c)| c != '\n' && c != '\r')
        .nth(column.saturating_sub(1))
        .map_or(line_end, |(at, _)| start + at)
}

/// The byte offset of the character after the one at `offset`, or
/// `offset` at the end of the text.
fn next_char(text: &str, offset: usize) -> usize {
    text[offset..]
        .chars()
        .next()
        .map_or(offset, |c| offset + c.len_utf8())
}

/// The position of `offset` in `text`, counted as [`SourceMap::position`]
/// counts it, for text that has no map.
fn position_in(text: &str, offset: usize) -> SourcePosition {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let (line, mut start) = line_of(text, offset);
    // A byte-order mark is not a column editors show.
    if start == 0 && text.starts_with('\u{feff}') {
        start = offset.min('\u{feff}'.len_utf8());
    }
    SourcePosition {
        offset,
        line,
        column: text[start..offset].chars().count() + 1,
    }
}

fn string(value: &Value, member: &str) -> Option<String> {
    value.get(member).and_then(Value::as_str).map(str::to_owned)
}

fn objects<'v>(value: &'v Value, member: &str) -> impl Iterator<Item = (usize, &'v Value)> {
    value
        .get(member)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, item)| item.is_object())
}

/// The outline of `value`, whatever its version.
fn outline(value: &Value, map: Option<&SourceMap>) -> Outline {
    let locate = |path: &str| map.and_then(|map| map.locate(path));
    let sources = objects(value, "sourceDescriptions")
        .map(|(index, source)| {
            let path = format!("#.sourceDescriptions[{index}]");
            SourceOutline {
                range: locate(&path),
                path,
                name: string(source, "name"),
                url: string(source, "url"),
                kind: string(source, "type"),
            }
        })
        .collect();
    let workflows = objects(value, "workflows")
        .map(|(index, workflow)| {
            let path = format!("#.workflows[{index}]");
            let steps = objects(workflow, "steps")
                .map(|(index, step)| {
                    let path = format!("{path}.steps[{index}]");
                    let target = [
                        ("operationId", StepTarget::OperationId as fn(String) -> _),
                        ("operationPath", StepTarget::OperationPath),
                        ("workflowId", StepTarget::WorkflowId),
                        ("channelPath", StepTarget::ChannelPath),
                    ]
                    .into_iter()
                    .find_map(|(member, target)| string(step, member).map(target));
                    StepOutline {
                        range: locate(&path),
                        path,
                        id: string(step, "stepId"),
                        target,
                    }
                })
                .collect();
            WorkflowOutline {
                range: locate(&path),
                id: string(workflow, "workflowId"),
                summary: string(workflow, "summary"),
                path,
                steps,
            }
        })
        .collect();
    let components = value
        .get("components")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(kind, _)| !kind.starts_with("x-"))
        .filter_map(|(kind, members)| Some((kind, members.as_object()?)))
        .flat_map(|(kind, members)| {
            members.keys().map(move |name| {
                let path = format!("#.components.{kind}.{name}");
                ComponentOutline {
                    range: locate(&path),
                    path,
                    kind: kind.clone(),
                    name: name.clone(),
                }
            })
        })
        .collect();
    Outline {
        title: value.get("info").and_then(|info| string(info, "title")),
        sources,
        workflows,
        components,
    }
}

/// One Arazzo version's model, as the checks need it.
trait Model: DeserializeOwned + Validate {
    type Version: DeserializeOwned;
    type Info: DeserializeOwned;
    type Source: DeserializeOwned;
    type Workflow: DeserializeOwned;
    type Components: DeserializeOwned;
    /// Whether the version has a root `$self`.
    const SELF: bool;
    fn unknown(&self, raw: &Value) -> Result<UnknownMembers, serde_json::Error>;
}

#[cfg(feature = "v1_0")]
impl Model for crate::v1_0::Description {
    type Version = crate::v1_0::Version;
    type Info = crate::v1_0::Info;
    type Source = crate::v1_0::SourceDescription;
    type Workflow = crate::v1_0::Workflow;
    type Components = crate::v1_0::Components;
    const SELF: bool = false;
    fn unknown(&self, raw: &Value) -> Result<UnknownMembers, serde_json::Error> {
        self.unknown_members(raw)
    }
}

#[cfg(feature = "v1_1")]
impl Model for crate::v1_1::Description {
    type Version = crate::v1_1::Version;
    type Info = crate::v1_1::Info;
    type Source = crate::v1_1::SourceDescription;
    type Workflow = crate::v1_1::Workflow;
    type Components = crate::v1_1::Components;
    const SELF: bool = true;
    fn unknown(&self, raw: &Value) -> Result<UnknownMembers, serde_json::Error> {
        self.unknown_members(raw)
    }
}

struct Checks<'a> {
    map: Option<&'a SourceMap>,
    diagnostics: &'a mut Vec<Diagnostic>,
}

impl Checks<'_> {
    fn push(&mut self, severity: Severity, path: String, key: bool, message: String) {
        let range = self.map.and_then(|map| {
            if key {
                map.locate_key(&path)
            } else {
                map.locate(&path)
            }
        });
        self.record(severity, path, range, message);
    }

    fn record(
        &mut self,
        severity: Severity,
        path: String,
        range: Option<SourceRange>,
        message: String,
    ) {
        self.diagnostics.push(Diagnostic {
            severity,
            path: Some(path),
            range,
            message,
        });
    }

    #[cfg_attr(not(any(feature = "v1_0", feature = "v1_1")), allow(unused_variables))]
    fn description(&mut self, value: &Value, options: EnumSet<ValidationOptions>) {
        if !value.is_object() {
            self.push(
                Severity::Error,
                "#".into(),
                false,
                "an Arazzo description is an object".into(),
            );
            return;
        }
        let Some(version) = value.get("arazzo") else {
            self.push(
                Severity::Error,
                "#".into(),
                false,
                "missing field `arazzo`".into(),
            );
            return;
        };
        let Some(version) = version.as_str() else {
            self.push(
                Severity::Error,
                "#.arazzo".into(),
                false,
                "`arazzo` is a version string, such as `1.1.0`".into(),
            );
            return;
        };
        if version.starts_with("1.0.") {
            #[cfg(feature = "v1_0")]
            return self.model::<crate::v1_0::Description>(value, options);
        } else if version.starts_with("1.1.") {
            #[cfg(feature = "v1_1")]
            return self.model::<crate::v1_1::Description>(value, options);
        } else {
            return self.push(
                Severity::Error,
                "#.arazzo".into(),
                false,
                format!("Arazzo `{version}` is not supported; 1.0.x and 1.1.x are"),
            );
        }
        #[allow(unreachable_code)]
        self.push(
            Severity::Error,
            "#.arazzo".into(),
            false,
            format!("support for Arazzo `{version}` is not enabled in this build"),
        );
    }

    /// Deserialize `value` as `T`; on failure, report where under `path`.
    fn part<T: DeserializeOwned>(&mut self, path: &str, value: &Value) -> bool {
        match serde_path_to_error::deserialize::<_, T>(value) {
            Ok(_) => true,
            Err(error) => {
                let (path, ends) = join(path, error.path());
                let range = self.map.and_then(|map| nearest(map, &path, &ends));
                self.record(Severity::Error, path, range, error.into_inner().to_string());
                false
            }
        }
    }

    fn model<M: Model>(&mut self, value: &Value, options: EnumSet<ValidationOptions>) {
        let mut clean = self.part::<M::Version>("#.arazzo", &value["arazzo"]);
        if M::SELF
            && let Some(self_) = value.get("$self")
        {
            // `null` is absent, as for the model.
            clean &= self.part::<Option<String>>("#.$self", self_);
        }
        for (member, required) in [
            ("info", true),
            ("sourceDescriptions", true),
            ("workflows", true),
            ("components", false),
        ] {
            let Some(part) = value.get(member) else {
                if required {
                    self.push(
                        Severity::Error,
                        "#".into(),
                        false,
                        format!("missing field `{member}`"),
                    );
                    clean = false;
                }
                continue;
            };
            let path = format!("#.{member}");
            clean &= match (member, part.as_array()) {
                ("info", _) => self.part::<M::Info>(&path, part),
                ("components", _) => self.part::<Option<M::Components>>(&path, part),
                ("sourceDescriptions", Some(items)) => self.items::<M::Source>(&path, items),
                ("workflows", Some(items)) => self.items::<M::Workflow>(&path, items),
                // Not a list: say so.
                ("sourceDescriptions", None) => self.part::<Vec<M::Source>>(&path, part),
                _ => self.part::<Vec<M::Workflow>>(&path, part),
            };
        }
        if !clean {
            return;
        }
        // Every part fits; the description as a whole should too.
        let description = match serde_path_to_error::deserialize::<_, M>(value) {
            Ok(description) => description,
            Err(error) => {
                let (path, ends) = join("#", error.path());
                let range = self.map.and_then(|map| nearest(map, &path, &ends));
                self.record(Severity::Error, path, range, error.into_inner().to_string());
                return;
            }
        };
        if let Err(errors) = description.validate(options) {
            for error in errors.errors {
                let key = error.is_about_key();
                self.push(Severity::Error, error.path, key, error.message);
            }
        }
        if let Ok(unknown) = description.unknown(value) {
            for member in unknown.members {
                let message = crate::unknown::message(
                    &member.name,
                    &unknown.version,
                    member.extensions_allowed,
                );
                self.push(Severity::Warning, member.path, true, message);
            }
        }
    }

    /// Each item on its own, so one bad item does not hide another.
    fn items<T: DeserializeOwned>(&mut self, path: &str, items: &[Value]) -> bool {
        let mut clean = true;
        for (index, item) in items.iter().enumerate() {
            clean &= self.part::<T>(&format!("{path}[{index}]"), item);
        }
        clean
    }
}

/// `base` followed by the model-style rendering of `path`, and where
/// each of its fields ends, outermost first. `base` is a path this module
/// builds, of plain names and indices.
fn join(base: &str, path: &serde_path_to_error::Path) -> (String, Vec<usize>) {
    use serde_path_to_error::Segment;
    let mut joined = base.to_owned();
    let mut ends: Vec<usize> = base.match_indices(['.', '[']).map(|(at, _)| at).collect();
    for segment in path.iter() {
        ends.push(joined.len());
        match segment {
            Segment::Seq { index } => joined.push_str(&format!("[{index}]")),
            // An externally tagged variant is a member named after it.
            Segment::Map { key } | Segment::Enum { variant: key } => {
                joined.push('.');
                joined.push_str(key);
            }
            Segment::Unknown => break,
        }
    }
    ends.push(joined.len());
    ends.dedup();
    (joined, ends)
}

/// Where `path` is written, or the nearest enclosing field that is, by
/// the field ends [`join`] recorded: a missing field is reported at the
/// object that lacks it, and a member name with a dot is never split.
fn nearest(map: &SourceMap, path: &str, ends: &[usize]) -> Option<SourceRange> {
    ends.iter().rev().find_map(|&end| map.locate(&path[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Document {
        Document::read(text, EnumSet::empty())
    }

    /// Severity, path and the line and column of each finding.
    type Found<'d> = (Severity, Option<&'d str>, Option<(usize, usize)>);

    fn found(document: &Document) -> Vec<Found<'_>> {
        document
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.severity,
                    diagnostic.path.as_deref(),
                    diagnostic
                        .range
                        .map(|range| (range.start.line, range.start.column)),
                )
            })
            .collect()
    }

    const VALID: &str = "arazzo: 1.1.0
info:
  title: Pets
  version: '1'
sourceDescriptions:
  - name: api
    url: api.yaml
    type: openapi
workflows:
  - workflowId: buy
    summary: Buy a pet
    steps:
      - stepId: find
        operationId: findPet
      - stepId: order
        operationPath: '{$sourceDescriptions.api.url}#/paths/~1order/post'
      - stepId: call
        workflowId: other
components:
  parameters:
    page: {name: page, in: query, value: 1}
  x-ext: {ignored: true}
";

    #[cfg(feature = "v1_1")]
    #[test]
    fn a_valid_description_has_an_outline_and_nothing_to_report() {
        let document = read(VALID);
        assert!(document.is_complete());
        assert!(!document.has_errors(), "{:?}", document.diagnostics());
        assert!(document.diagnostics().is_empty());
        assert_eq!(document.version(), Some("1.1.0"));
        assert_eq!(document.text(), VALID);
        assert!(document.source_map().is_some());
        let outline = document.outline().unwrap();
        assert_eq!(outline.title.as_deref(), Some("Pets"));
        assert_eq!(
            outline.sources,
            [SourceOutline {
                path: "#.sourceDescriptions[0]".into(),
                range: outline.sources[0].range,
                name: Some("api".into()),
                url: Some("api.yaml".into()),
                kind: Some("openapi".into()),
            }]
        );
        assert_eq!(outline.sources[0].range.unwrap().start.line, 6);
        let workflow = &outline.workflows[0];
        assert_eq!(workflow.id.as_deref(), Some("buy"));
        assert_eq!(workflow.summary.as_deref(), Some("Buy a pet"));
        let targets: Vec<_> = workflow
            .steps
            .iter()
            .map(|step| step.target.clone())
            .collect();
        assert_eq!(
            targets,
            [
                Some(StepTarget::OperationId("findPet".into())),
                Some(StepTarget::OperationPath(
                    "{$sourceDescriptions.api.url}#/paths/~1order/post".into()
                )),
                Some(StepTarget::WorkflowId("other".into())),
            ]
        );
        assert_eq!(workflow.steps[1].path, "#.workflows[0].steps[1]");
        assert_eq!(workflow.steps[1].range.unwrap().start.line, 15);
        assert_eq!(
            outline.components,
            [ComponentOutline {
                path: "#.components.parameters.page".into(),
                range: outline.components[0].range,
                kind: "parameters".into(),
                name: "page".into(),
            }]
        );
        assert_eq!(outline.components[0].range.unwrap().start.line, 21);
    }

    #[cfg(feature = "v1_1")]
    #[test]
    fn every_broken_part_is_reported_and_the_rest_outlined() {
        let text = VALID
            .replace("title: Pets", "title: [Pets]")
            .replace("url: api.yaml", "url: 7")
            .replace("stepId: order", "stepId: {order: 1}");
        let document = read(&text);
        assert!(document.is_complete());
        assert_eq!(
            found(&document),
            [
                (Severity::Error, Some("#.info.title"), Some((3, 10))),
                (
                    Severity::Error,
                    Some("#.sourceDescriptions[0].url"),
                    Some((7, 10))
                ),
                (
                    Severity::Error,
                    Some("#.workflows[0].steps[1].stepId"),
                    Some((15, 17))
                ),
            ]
        );
        // What is still well-formed is still outlined.
        let outline = document.outline().unwrap();
        assert_eq!(outline.title, None);
        assert_eq!(outline.sources[0].url, None);
        assert_eq!(outline.sources[0].name.as_deref(), Some("api"));
        assert_eq!(outline.workflows[0].steps[1].id, None);
        assert_eq!(outline.workflows[0].steps[2].id.as_deref(), Some("call"));
    }

    #[cfg(feature = "v1_1")]
    #[test]
    fn a_description_that_fits_is_validated_and_its_unknown_members_listed() {
        let text = VALID
            .replace("title: Pets", "title: ''")
            .replace("    summary: Buy a pet", "    sumary: Buy a pet");
        let document = read(&text);
        assert_eq!(
            found(&document),
            [
                (Severity::Error, Some("#.info.title"), Some((3, 10))),
                (
                    Severity::Warning,
                    Some("#.workflows[0].sumary"),
                    Some((11, 5))
                ),
            ]
        );
        assert!(document.diagnostics()[1].message.contains("`sumary`"));
        // Validation options apply.
        let document = Document::read(
            text.as_str(),
            ValidationOptions::IgnoreEmptyInfoTitle.into(),
        );
        assert_eq!(found(&document).len(), 1);
        assert!(!document.has_errors());
    }

    #[cfg(feature = "v1_1")]
    #[test]
    fn a_name_outside_the_key_pattern_is_located_at_the_name() {
        let text = VALID.replace("    page:", "    'bad key':");
        let document = read(&text);
        assert_eq!(
            found(&document),
            [(
                Severity::Error,
                Some("#.components.parameters.bad key"),
                Some((21, 5))
            )]
        );
    }

    #[cfg(feature = "v1_1")]
    #[test]
    fn missing_parts_are_reported_at_the_object_that_lacks_them() {
        let document = read("arazzo: 1.1.0\ninfo: {title: T}\n");
        assert_eq!(
            found(&document),
            [
                (Severity::Error, Some("#.info"), Some((2, 7))),
                (Severity::Error, Some("#"), Some((1, 1))),
                (Severity::Error, Some("#"), Some((1, 1))),
            ]
        );
        let messages: Vec<_> = document
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect();
        assert_eq!(
            messages,
            [
                "missing field `version`",
                "missing field `sourceDescriptions`",
                "missing field `workflows`",
            ]
        );
    }

    #[cfg(feature = "v1_1")]
    #[test]
    fn parts_that_are_not_lists_and_a_bad_self_are_reported() {
        let text = VALID
            .replace("arazzo: 1.1.0", "arazzo: 1.1.0\n$self: 5")
            .replace("sourceDescriptions:\n", "sourceDescriptions: 3\nx-was:\n")
            .replace("workflows:\n", "workflows: {}\nx-old:\n");
        let document = read(&text);
        let paths: Vec<_> = found(&document)
            .into_iter()
            .map(|(_, path, _)| path)
            .collect();
        assert_eq!(
            paths,
            [
                Some("#.$self"),
                Some("#.sourceDescriptions"),
                Some("#.workflows")
            ]
        );
    }

    #[cfg(feature = "v1_1")]
    #[test]
    fn a_version_the_model_refuses_is_reported() {
        // `arazzo: 1.1.x` with a suffix the version type refuses.
        let document = read(&VALID.replace("arazzo: 1.1.0", "arazzo: 1.1.x"));
        assert_eq!(found(&document)[0].1, Some("#.arazzo"));
        assert_eq!(document.version(), Some("1.1.x"));
    }

    #[test]
    fn versions_it_cannot_check_are_said_so() {
        for (text, path, message) in [
            ("[1, 2]", "#", "an Arazzo description is an object"),
            ("info: {}", "#", "missing field `arazzo`"),
            (
                "arazzo: 1",
                "#.arazzo",
                "`arazzo` is a version string, such as `1.1.0`",
            ),
            (
                "arazzo: 2.0.0",
                "#.arazzo",
                "Arazzo `2.0.0` is not supported; 1.0.x and 1.1.x are",
            ),
        ] {
            let document = read(text);
            assert!(document.is_complete());
            assert_eq!(document.diagnostics().len(), 1, "{text}");
            let diagnostic = &document.diagnostics()[0];
            assert_eq!(diagnostic.path.as_deref(), Some(path), "{text}");
            assert_eq!(diagnostic.message, message, "{text}");
            assert!(diagnostic.range.is_some(), "{text}");
        }
        assert!(read("[1, 2]").outline().is_none());
    }

    #[cfg(feature = "v1_0")]
    #[test]
    fn a_v1_0_description_is_checked_against_its_own_model() {
        let text = VALID.replace("arazzo: 1.1.0", "arazzo: 1.0.1").replace(
            "      - stepId: call\n        workflowId: other\n",
            "      - stepId: call\n        workflowId: other\n        timeout: 5\n",
        );
        let document = read(&text);
        assert_eq!(document.version(), Some("1.0.1"));
        // `timeout` is a v1.1 field: unknown to v1.0.
        assert_eq!(
            found(&document),
            [(
                Severity::Warning,
                Some("#.workflows[0].steps[2].timeout"),
                Some((19, 9))
            )]
        );
        assert!(document.diagnostics()[0].message.contains("Arazzo 1.0.1"));
        let document = read(&text.replace("type: openapi", "type: asyncapi"));
        assert_eq!(found(&document)[0].1, Some("#.sourceDescriptions[0].type"));
    }

    #[test]
    fn a_syntax_error_keeps_the_outline_of_the_text_above_it() {
        let text = "arazzo: 1.1.0
info: {title: Pets, version: '1'}
sourceDescriptions: [{name: api, url: api.yaml}]
workflows:
  - workflowId: buy
    steps:
      - stepId: find
        operationId: findPet
  - workflowId: \"unterminated
    steps: []
";
        let document = read(text);
        assert!(!document.is_complete());
        assert!(document.has_errors());
        assert_eq!(document.diagnostics().len(), 1);
        let syntax = &document.diagnostics()[0];
        assert_eq!(syntax.path, None);
        assert!(syntax.range.unwrap().start.line >= 9, "{syntax:?}");
        assert_eq!(
            syntax.message,
            "invalid indentation in multiline quoted scalar"
        );
        let outline = document.outline().unwrap();
        assert_eq!(outline.workflows.len(), 1);
        assert_eq!(outline.workflows[0].steps[0].id.as_deref(), Some("find"));
        assert_eq!(outline.workflows[0].range.unwrap().start.line, 5);
        // The map is of the text that was read.
        assert!(document.source_map().unwrap().text().len() < text.len());
    }

    #[test]
    fn broken_json_has_a_located_error_and_no_outline() {
        let text = "{\"arazzo\": \"1.1.0\",\n \"info\": {\"title\": \"T\"\n";
        let document = read(text);
        assert!(!document.is_complete());
        assert!(document.outline().is_none());
        assert!(document.source_map().is_none());
        assert_eq!(document.version(), None);
        let range = document.diagnostics()[0].range.unwrap();
        assert!(range.start.line >= 2, "{range:?}");
        // A syntax error on the first line leaves nothing to read again.
        let document = read("{");
        assert!(document.outline().is_none());
        assert_eq!(document.diagnostics()[0].range.unwrap().start.line, 1);
    }

    #[test]
    fn positions_of_text_without_a_map() {
        let text = "\u{feff}ab\ncé\nx";
        assert_eq!(
            (position_in(text, 3).line, position_in(text, 3).column),
            (1, 1)
        );
        let at = text.find('é').unwrap();
        // Inside `é`: back to its start.
        let inside = position_in(text, at + 1);
        assert_eq!((inside.offset, inside.line, inside.column), (at, 2, 2));
        assert_eq!(next_char(text, at), at + 'é'.len_utf8());
        assert_eq!(next_char(text, text.len()), text.len());
        assert_eq!(position_in(text, 99).line, 3);
    }

    #[test]
    fn a_path_not_written_is_located_at_its_nearest_written_parent() {
        let map = SourceMap::parse("a:\n  b: 1\n").unwrap();
        let b = map.locate("#.a.b").unwrap();
        assert_eq!(nearest(&map, "#.a.b", &[1, 3, 5]), Some(b));
        assert_eq!(
            nearest(&map, "#.a.b.c[0].d", &[1, 3, 5, 7, 10, 12]),
            Some(b)
        );
        assert_eq!(nearest(&map, "#.z", &[1, 3]), map.locate("#"));
        assert_eq!(nearest(&map, "nothing", &[7]), None);
    }

    #[cfg(feature = "v1_1")]
    #[test]
    fn a_member_name_with_a_dot_is_not_split_to_find_a_parent() {
        // Parameter `a.b` lacks `name`; `a` is a different parameter with a
        // member `b`, so `#.components.parameters.a.b` names two places.
        let text = VALID.replace(
            "    page: {name: page, in: query, value: 1}",
            "    a: {name: a, in: query, value: 1, b: 2}\n    a.b: {in: query, value: 1}",
        );
        let document = read(&text);
        let found = found(&document);
        assert_eq!(found[0].1, Some("#.components.parameters.a.b"), "{found:?}");
        // Not the unrelated `a` on line 21: the `parameters` map it is in.
        assert_eq!(found[0].2, Some((21, 5)), "{found:?}");
    }

    #[cfg(feature = "v1_1")]
    #[test]
    fn optional_parts_written_as_null_are_absent() {
        let text = VALID
            .replace("arazzo: 1.1.0", "arazzo: 1.1.0\n$self: null")
            .replace("components:\n", "x-was:\n")
            + "components: null\n";
        let document = read(&text);
        assert!(
            document.diagnostics().is_empty(),
            "{:?}",
            document.diagnostics()
        );
    }

    #[test]
    fn crlf_text_and_a_byte_order_mark_are_located_like_the_map() {
        let text = "\u{feff}arazzo: 1.1.0\r\ninfo: {title: [x]}\r\n";
        let document = read(text);
        let map = document.source_map().unwrap();
        let title = map.locate("#.info.title").unwrap();
        assert_eq!((title.start.line, title.start.column), (2, 15));
        assert_eq!(document.utf16_column(title.start), 15);
        // A syntax error on the first line, after a byte-order mark.
        let document = read("\u{feff}a: b: c\n");
        let start = document.diagnostics()[0].range.unwrap().start;
        let plain = read("a: b: c\n").diagnostics()[0].range.unwrap().start;
        assert_eq!((start.line, start.column), (plain.line, plain.column));
        assert_eq!(offset_of("\u{feff}ab", 1, 2), 4);
    }

    #[test]
    fn recovery_gives_up_after_its_line_budget() {
        // Every line is inside an unclosed flow sequence, which ends far
        // more lines below its start than recovery tries.
        let text = format!("a: [\n{}", "1,\n".repeat(RECOVERY_LINES + 4));
        let document = read(&text);
        assert!(!document.is_complete());
        assert_eq!(document.outline(), None);
    }

    #[test]
    fn text_that_is_json_is_read_as_json_and_yaml_keys_must_not_collide() {
        // Duplicate JSON keys: the last wins, as `serde_json` reads them.
        let document = read("{\"arazzo\": \"1.0.0\", \"arazzo\": \"2.0.0\"}");
        assert_eq!(document.version(), Some("2.0.0"));
        // YAML keys `200` and `'200'` are one JSON key: refused.
        let document = read("a:\n  200: x\n  '200': y\n");
        assert!(!document.is_complete());
        assert!(document.has_errors());
    }

    #[test]
    fn a_bare_carriage_return_ends_a_line() {
        let document = read("a: b\rc: '😀x'\r");
        let map = document.source_map().unwrap();
        let x = map.position(document.text().find('x').unwrap());
        assert_eq!((x.line, x.column, document.utf16_column(x)), (2, 6, 7));
        assert_eq!(map.utf16_column(x), 7);
        let text = "a: 1\rb: \"open\rc: 2\r";
        let document = read(text);
        // The first line is read again: an object, with nothing to outline.
        assert_eq!(document.outline(), Some(&Outline::default()));
        assert!(!document.is_complete());
        let start = document.diagnostics()[0].range.unwrap().start;
        assert!(start.line >= 2, "{start:?}");
        assert_eq!(offset_of(text, 2, 1), 5);
        assert_eq!(offset_of(text, 2, 99), text[5..].find('\r').unwrap() + 5);
        // Recovery steps back over bare carriage returns too.
        let recovered = read("a: 1\rb: [1\r");
        assert!(!recovered.is_complete());
    }

    #[test]
    fn line_and_column_to_offset() {
        let text = "ab\ncé\nx";
        assert_eq!(offset_of(text, 1, 1), 0);
        assert_eq!(offset_of(text, 2, 2), 4);
        assert_eq!(offset_of(text, 2, 3), 6);
        // Past the end of a line, or of the text.
        assert_eq!(offset_of(text, 2, 9), 6);
        assert_eq!(offset_of(text, 9, 1), text.len());
    }

    #[test]
    fn a_syntax_reason_without_its_location() {
        let error = crate::source_map::parse::value("a: [").unwrap_err();
        assert!(!reason(&error).starts_with("error"), "{}", reason(&error));
        assert!(!reason(&error).contains('\n'));
        // A message without a location prefix is kept as it is.
        let error = crate::source_map::parse::value("a: 1\na: 2\n").unwrap_err();
        assert!(!reason(&error).is_empty());
        let error = crate::source_map::parse::value("a: 1\rb: [1\r").unwrap_err();
        assert!(!reason(&error).contains(" at line "), "{}", reason(&error));
    }

    #[cfg(not(feature = "v1_0"))]
    #[test]
    fn a_version_this_build_cannot_check_is_said_so() {
        let document = read("arazzo: 1.0.1\n");
        assert_eq!(
            document.diagnostics()[0].message,
            "support for Arazzo `1.0.1` is not enabled in this build"
        );
    }

    #[cfg(not(feature = "v1_1"))]
    #[test]
    fn a_v1_1_description_in_a_build_without_it_is_said_so() {
        let document = read("arazzo: 1.1.0\n");
        assert_eq!(
            document.diagnostics()[0].message,
            "support for Arazzo `1.1.0` is not enabled in this build"
        );
    }

    #[test]
    fn utf16_columns_count_code_units() {
        let document = read("a: '😀x'\n");
        let map = document.source_map().unwrap();
        let x = map.position(document.text().find('x').unwrap());
        assert_eq!(x.column, 6);
        assert_eq!(document.utf16_column(x), 7);
        assert_eq!(map.utf16_column(x), 7);
        // A byte-order mark is not counted.
        let document = read("\u{feff}a: x\n");
        let map = document.source_map().unwrap();
        let x = map.position(document.text().find('x').unwrap());
        assert_eq!((x.column, document.utf16_column(x)), (4, 4));
    }
}
