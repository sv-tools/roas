//! Which Path Item Object a concrete request path belongs to.
//!
//! Every implementation of this idea needs a router, and none of them
//! can borrow the framework's: `kin-openapi` makes the caller pass one
//! in, `libopenapi-validator` ships its own. So does this — a Path
//! Templating matcher is small, and the alternatives disagree with the
//! specification in ways that matter.
//!
//! `matchit`, the router axum uses, spells parameters `{name}` exactly
//! as [Path Templating](https://spec.openapis.org/oas/v3.2.0#path-templating)
//! does and orders static above dynamic exactly as OpenAPI's "concrete
//! before templated" rule does — but it rejects two parameters in one
//! segment, which the specification permits (`/{a}-{b}`), and it has no
//! notion of a Server Object's base path. Both matter here, so the
//! matcher below is its own.

use std::collections::BTreeMap;

use roas::v3_2::path_item::Paths;
use roas::v3_2::server::Server;

use crate::request::decode_path_segment;

/// The path templates of one description, matched against real paths.
#[derive(Clone, Debug, Default)]
pub(crate) struct Router {
    routes: Vec<Route>,
    /// Server base paths, longest first, each tried as a prefix to strip.
    base_paths: Vec<String>,
}

/// What matching a path produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Match<'r> {
    /// The template as the description spells it, e.g. `/pets/{petId}`.
    pub(crate) template: &'r str,
    /// Path parameters, decoded, in the order the template names them.
    pub(crate) parameters: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
struct Route {
    template: String,
    segments: Vec<Segment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    /// A segment with no `{`, matched by equality.
    Literal(String),
    /// A segment holding at least one template expression.
    Parts(Vec<Part>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Literal(String),
    Parameter(String),
}

impl Router {
    /// Build a router over a description's paths.
    ///
    /// `base_path` overrides what the Server Objects imply; pass `None`
    /// to derive the prefixes from `servers` instead.
    pub(crate) fn new(paths: &Paths, servers: Option<&[Server]>, base_path: Option<&str>) -> Self {
        let routes = paths
            .iter()
            .map(|(template, _)| Route::parse(template))
            .collect();

        let mut base_paths = match base_path {
            Some(base) => vec![normalize_base(base)],
            None => servers
                .unwrap_or_default()
                .iter()
                .filter_map(server_base_path)
                .collect(),
        };
        base_paths.retain(|base| !base.is_empty());
        base_paths.sort_by_key(|base| std::cmp::Reverse(base.len()));
        base_paths.dedup();

        Self { routes, base_paths }
    }

    /// The template `path` belongs to, or `None` when the description
    /// describes no such path.
    ///
    /// A server base path is stripped when the request carries one, and
    /// the unstripped path is tried as well — an application mounted
    /// behind a proxy sees the path without the prefix its own
    /// description advertises.
    pub(crate) fn route(&self, path: &str) -> Option<Match<'_>> {
        for candidate in self.candidates(path) {
            if let Some(matched) = self.best_match(candidate) {
                return Some(matched);
            }
        }
        None
    }

    /// `path` with each server base path stripped, longest first, and
    /// then `path` itself.
    fn candidates<'p>(&self, path: &'p str) -> impl Iterator<Item = &'p str> {
        let stripped: Vec<&'p str> = self
            .base_paths
            .iter()
            .filter_map(|base| strip_base(path, base))
            .collect();
        stripped.into_iter().chain(std::iter::once(path))
    }

    /// The most specific route that matches, per the specification's
    /// rule that a concrete segment outranks a templated one.
    fn best_match(&self, path: &str) -> Option<Match<'_>> {
        let mut best: Option<(&Route, BTreeMap<String, String>)> = None;
        for route in &self.routes {
            let Some(parameters) = route.match_path(path) else {
                continue;
            };
            let better = match &best {
                None => true,
                Some((incumbent, _)) => route.outranks(incumbent),
            };
            if better {
                best = Some((route, parameters));
            }
        }
        best.map(|(route, parameters)| Match {
            template: route.template.as_str(),
            parameters,
        })
    }
}

impl Route {
    fn parse(template: &str) -> Self {
        let segments = template
            .split('/')
            .map(|segment| {
                if segment.contains('{') {
                    Segment::Parts(parse_parts(segment))
                } else {
                    Segment::Literal(segment.to_owned())
                }
            })
            .collect();
        Self {
            template: template.to_owned(),
            segments,
        }
    }

    /// The path parameters `path` supplies, or `None` when it does not
    /// match. OpenAPI has no wildcard, so the segment counts must agree.
    fn match_path(&self, path: &str) -> Option<BTreeMap<String, String>> {
        let segments: Vec<&str> = path.split('/').collect();
        if segments.len() != self.segments.len() {
            return None;
        }

        let mut parameters = BTreeMap::new();
        for (template, actual) in self.segments.iter().zip(segments) {
            match template {
                Segment::Literal(literal) => {
                    if decode_path_segment(actual) != *literal {
                        return None;
                    }
                }
                Segment::Parts(parts) => match_parts(parts, actual, &mut parameters)?,
            }
        }
        Some(parameters)
    }

    /// Whether this route is more specific than `other`, comparing
    /// segment by segment: the first position where one is literal and
    /// the other is templated decides it.
    fn outranks(&self, other: &Route) -> bool {
        for (mine, theirs) in self.segments.iter().zip(&other.segments) {
            let mine_literal = matches!(mine, Segment::Literal(_));
            let theirs_literal = matches!(theirs, Segment::Literal(_));
            if mine_literal != theirs_literal {
                return mine_literal;
            }
        }
        false
    }
}

/// Split a segment into its literal and `{name}` parts. An unclosed `{`
/// is treated as a literal — the description is malformed, and
/// `roas`'s own validator is where that gets reported.
fn parse_parts(segment: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut rest = segment;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|at| open + at) else {
            break;
        };
        if open > 0 {
            parts.push(Part::Literal(rest[..open].to_owned()));
        }
        parts.push(Part::Parameter(rest[open + 1..close].to_owned()));
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        parts.push(Part::Literal(rest.to_owned()));
    }
    parts
}

/// Match one templated segment, filling in the parameters it names.
///
/// A parameter is matched non-greedily up to the literal that follows
/// it, so `/{a}-{b}` reads `x-y-z` as `a = x`, `b = y-z`. There is no
/// backtracking: the specification does not define what a segment with
/// several parameters and ambiguous separators means, and guessing
/// would make a validator's verdict depend on the guess.
fn match_parts(
    parts: &[Part],
    segment: &str,
    parameters: &mut BTreeMap<String, String>,
) -> Option<()> {
    let mut rest = segment;
    let mut index = 0;
    while index < parts.len() {
        match &parts[index] {
            Part::Literal(literal) => {
                rest = rest.strip_prefix(literal.as_str())?;
            }
            Part::Parameter(name) => {
                let value = match parts.get(index + 1) {
                    // Trailing parameter: it takes what is left.
                    None => {
                        let value = rest;
                        rest = "";
                        value
                    }
                    // Followed by a literal: up to that literal's first
                    // occurrence. Two adjacent parameters cannot be told
                    // apart, so the description is rejected there.
                    Some(Part::Literal(literal)) => {
                        let at = rest.find(literal.as_str())?;
                        let value = &rest[..at];
                        rest = &rest[at..];
                        value
                    }
                    Some(Part::Parameter(_)) => return None,
                };
                // An empty path parameter is not a value: `/pets/` does
                // not supply a `petId`.
                if value.is_empty() {
                    return None;
                }
                parameters.insert(name.clone(), decode_path_segment(value));
            }
        }
        index += 1;
    }
    rest.is_empty().then_some(())
}

/// The path component of a Server Object's URL, with any server
/// variables replaced by their defaults.
fn server_base_path(server: &Server) -> Option<String> {
    let url = resolve_server_variables(server)?;
    let path = match url.split_once("://") {
        // Absolute: everything from the first `/` after the authority.
        Some((_, authority_and_path)) => match authority_and_path.find('/') {
            Some(at) => &authority_and_path[at..],
            None => "",
        },
        // Relative server URLs are already a path.
        None => url.as_str(),
    };
    Some(normalize_base(path))
}

/// A server URL with `{variable}` substituted from the Server Variable
/// defaults, or `None` when a variable has no default to substitute.
///
/// `ServerVariable`'s fields are private in `roas` with no accessors, so
/// the defaults are read back through its `Serialize` impl rather than
/// from the struct.
fn resolve_server_variables(server: &Server) -> Option<String> {
    if !server.url.contains('{') {
        return Some(server.url.clone());
    }
    let variables = server.variables.as_ref()?;
    let mut url = server.url.clone();
    for (name, variable) in variables {
        let value = serde_json::to_value(variable).ok()?;
        let default = value.get("default")?.as_str()?;
        url = url.replace(&format!("{{{name}}}"), default);
    }
    (!url.contains('{')).then_some(url)
}

/// A base path with a leading slash and no trailing one, so that
/// stripping it from a request path leaves a path that still starts
/// with `/`.
fn normalize_base(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.starts_with('/') {
        trimmed.to_owned()
    } else {
        format!("/{trimmed}")
    }
}

/// `path` without `base`, but only when `base` covers whole segments —
/// `/v1` is a prefix of `/v10/pets` as a string and not as a path.
fn strip_base<'p>(path: &'p str, base: &str) -> Option<&'p str> {
    let rest = path.strip_prefix(base)?;
    if rest.is_empty() {
        Some("/")
    } else if rest.starts_with('/') {
        Some(rest)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roas::v3_2::path_item::PathItem;

    fn router(templates: &[&str]) -> Router {
        let paths: Paths = templates
            .iter()
            .map(|template| ((*template).to_owned(), PathItem::default()))
            .collect::<Vec<_>>()
            .into();
        Router::new(&paths, None, None)
    }

    fn matched<'r>(router: &'r Router, path: &str) -> (&'r str, Vec<(String, String)>) {
        let found = router.route(path).expect("path must match");
        (found.template, found.parameters.into_iter().collect())
    }

    #[test]
    fn a_literal_path_matches_itself() {
        let router = router(&["/pets"]);
        assert_eq!(matched(&router, "/pets"), ("/pets", vec![]));
        assert!(router.route("/pet").is_none());
        assert!(router.route("/pets/7").is_none());
    }

    #[test]
    fn a_template_captures_its_parameter() {
        let router = router(&["/pets/{petId}"]);
        assert_eq!(
            matched(&router, "/pets/7"),
            ("/pets/{petId}", vec![("petId".to_owned(), "7".to_owned())]),
        );
    }

    #[test]
    fn a_concrete_path_outranks_a_templated_one() {
        let router = router(&["/pets/{petId}", "/pets/mine"]);
        assert_eq!(matched(&router, "/pets/mine").0, "/pets/mine");
        assert_eq!(matched(&router, "/pets/7").0, "/pets/{petId}");
    }

    #[test]
    fn the_first_differing_segment_decides_which_route_is_more_specific() {
        let router = router(&["/{a}/b/{c}", "/a/{b}/{c}"]);
        assert_eq!(matched(&router, "/a/b/c").0, "/a/{b}/{c}");
    }

    #[test]
    fn a_segment_may_hold_more_than_one_parameter() {
        let router = router(&["/reports/{year}-{month}"]);
        assert_eq!(
            matched(&router, "/reports/2026-08"),
            (
                "/reports/{year}-{month}",
                vec![
                    ("month".to_owned(), "08".to_owned()),
                    ("year".to_owned(), "2026".to_owned())
                ],
            ),
        );
    }

    #[test]
    fn a_parameter_may_sit_inside_a_segment() {
        let router = router(&["/report.{format}"]);
        assert_eq!(
            matched(&router, "/report.json"),
            (
                "/report.{format}",
                vec![("format".to_owned(), "json".to_owned())]
            ),
        );
        assert!(router.route("/report.").is_none());
    }

    #[test]
    fn two_adjacent_parameters_cannot_be_told_apart_so_nothing_matches() {
        let router = router(&["/{a}{b}"]);
        assert!(router.route("/xy").is_none());
    }

    #[test]
    fn a_path_parameter_is_percent_decoded_after_the_split() {
        let router = router(&["/pets/{name}"]);
        assert_eq!(
            matched(&router, "/pets/rex%20the%20dog").1,
            vec![("name".to_owned(), "rex the dog".to_owned())],
        );
    }

    #[test]
    fn an_encoded_slash_stays_inside_one_parameter() {
        let router = router(&["/files/{path}"]);
        assert_eq!(
            matched(&router, "/files/a%2Fb").1,
            vec![("path".to_owned(), "a/b".to_owned())],
        );
    }

    #[test]
    fn an_empty_parameter_is_not_a_value() {
        let router = router(&["/pets/{petId}"]);
        assert!(router.route("/pets/").is_none());
    }

    #[test]
    fn a_server_base_path_is_stripped_before_matching() {
        let paths: Paths = vec![("/pets".to_owned(), PathItem::default())].into();
        let servers = [Server {
            url: "https://api.example.com/v1".to_owned(),
            ..Server::default()
        }];
        let router = Router::new(&paths, Some(&servers), None);
        assert_eq!(matched(&router, "/v1/pets").0, "/pets");
        // A proxy may already have stripped it.
        assert_eq!(matched(&router, "/pets").0, "/pets");
    }

    #[test]
    fn a_base_path_only_strips_whole_segments() {
        let paths: Paths = vec![("/pets".to_owned(), PathItem::default())].into();
        let servers = [Server {
            url: "/v1".to_owned(),
            ..Server::default()
        }];
        let router = Router::new(&paths, Some(&servers), None);
        assert!(router.route("/v10/pets").is_none());
    }

    #[test]
    fn an_explicit_base_path_overrides_the_servers() {
        let paths: Paths = vec![("/pets".to_owned(), PathItem::default())].into();
        let servers = [Server {
            url: "https://api.example.com/v1".to_owned(),
            ..Server::default()
        }];
        let router = Router::new(&paths, Some(&servers), Some("api"));
        assert_eq!(matched(&router, "/api/pets").0, "/pets");
        assert!(router.route("/v1/pets").is_none());
    }

    #[test]
    fn a_server_variable_is_replaced_by_its_default() {
        let variables = serde_json::from_value(serde_json::json!({
            "version": { "default": "v2", "enum": ["v1", "v2"] }
        }))
        .expect("server variables must parse");
        let paths: Paths = vec![("/pets".to_owned(), PathItem::default())].into();
        let servers = [Server {
            url: "https://api.example.com/{version}".to_owned(),
            variables: Some(variables),
            ..Server::default()
        }];
        let router = Router::new(&paths, Some(&servers), None);
        assert_eq!(matched(&router, "/v2/pets").0, "/pets");
    }

    #[test]
    fn a_server_with_a_variable_and_no_definition_contributes_no_base_path() {
        let paths: Paths = vec![("/pets".to_owned(), PathItem::default())].into();
        let servers = [Server {
            url: "https://api.example.com/{version}".to_owned(),
            ..Server::default()
        }];
        let router = Router::new(&paths, Some(&servers), None);
        assert_eq!(matched(&router, "/pets").0, "/pets");
        assert!(router.route("/v1/pets").is_none());
    }

    #[test]
    fn a_server_without_a_path_contributes_no_prefix() {
        let paths: Paths = vec![("/pets".to_owned(), PathItem::default())].into();
        let servers = [Server {
            url: "https://api.example.com".to_owned(),
            ..Server::default()
        }];
        let router = Router::new(&paths, Some(&servers), None);
        assert_eq!(matched(&router, "/pets").0, "/pets");
    }

    #[test]
    fn a_router_over_no_paths_matches_nothing() {
        assert!(router(&[]).route("/pets").is_none());
    }

    #[test]
    fn an_unclosed_brace_is_a_literal() {
        let router = router(&["/pets/{petId"]);
        assert_eq!(matched(&router, "/pets/{petId").0, "/pets/{petId");
    }
}
