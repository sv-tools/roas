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

use roas::v3_2::operation::Operation;
use roas::v3_2::path_item::PathItem;
use roas::v3_2::server::Server;

use crate::request::decode_path_segment;

/// The path templates of one description, matched against real paths.
#[derive(Clone, Debug, Default)]
pub(crate) struct Router {
    routes: Vec<Route>,
}

/// What matching a path produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Match<'r> {
    /// The template as the description spells it, e.g. `/pets/{petId}`.
    pub(crate) template: &'r str,
    /// Path parameters exactly as they arrived, still percent-encoded.
    ///
    /// Decoding happens after `style` has split them, not before: in a
    /// non-exploded array `a%2Cb` is one item containing a comma, and
    /// decoding first would turn the delimiter into a separator.
    pub(crate) parameters: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
struct Route {
    template: String,
    segments: Vec<Segment>,
    /// The base paths each operation may arrive under, keyed as the
    /// Path Item Object keys it, longest first.
    ///
    /// Per operation rather than per route: a Server Object may sit on
    /// the Operation Object as well as the Path Item Object and the root,
    /// and the innermost one wins
    /// ([§4.8.5](https://spec.openapis.org/oas/v3.2.0#operation-object)).
    /// One route can therefore serve `GET` under `/v1` and `POST` under
    /// `/v2` — and `GET /v2/pets` must *not* match, which is why the
    /// method reaches the router rather than being applied afterwards.
    method_bases: BTreeMap<String, Vec<String>>,
    /// The same, for `additionalOperations`, keyed by the method name
    /// as written. Kept apart from `method_bases` so neither map is
    /// ever searched with the other's key.
    additional_bases: BTreeMap<String, Vec<String>>,
    /// The base paths for a method this Path Item Object does not
    /// describe: every prefix any of its operations answers under, plus
    /// the inherited ones.
    ///
    /// A described method is confined to its own prefix — that is the
    /// point of keeping them apart — but an *undescribed* one is a
    /// different question. `DELETE /v2/pets` where only `GET` lives
    /// under `/v2` is a real path being asked for a method it does not
    /// have, so matching it broadly is what turns a misleading "no such
    /// path" into "no such method here".
    undescribed_bases: Vec<String>,
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
    /// `base_path` overrides every Server Object; pass `None` to derive
    /// each operation's prefixes from the servers that apply to it.
    ///
    /// Takes Path Item Objects that [`crate::paths::resolve`] has
    /// already followed and merged, because the Server Objects that
    /// decide where a route lives can be on the far side of a `$ref`.
    pub(crate) fn new(
        paths: &BTreeMap<String, PathItem>,
        servers: Option<&[Server]>,
        base_path: Option<&str>,
    ) -> Self {
        let override_base = base_path.map(|base| vec![normalize_base(base)]);
        let routes = paths
            .iter()
            .map(|(template, path_item)| {
                let inherited = path_item.servers.as_deref().or(servers);
                let (method_bases, additional_bases, inherited_bases) = match &override_base {
                    Some(base) => (BTreeMap::new(), BTreeMap::new(), base.clone()),
                    None => (
                        base_paths_per_operation(path_item.operations.as_ref(), inherited),
                        base_paths_per_operation(
                            path_item.additional_operations.as_ref(),
                            inherited,
                        ),
                        base_paths_of(inherited),
                    ),
                };
                let undescribed_bases = method_bases
                    .values()
                    .chain(additional_bases.values())
                    .flatten()
                    .cloned()
                    .chain(inherited_bases)
                    .collect();
                Route::parse(template, method_bases, additional_bases, undescribed_bases)
            })
            .collect();
        Self { routes }
    }

    /// The template `path` belongs to when it carries `method`, or
    /// `None` when the description describes no such path.
    ///
    /// The route's base path for *that method* is stripped when the
    /// request carries one, and the unstripped path is tried as well —
    /// an application mounted behind a proxy sees the path without the
    /// prefix its own description advertises.
    pub(crate) fn route(&self, path: &str, method: &str) -> Option<Match<'_>> {
        let mut best: Option<(&Route, BTreeMap<String, String>)> = None;
        for route in &self.routes {
            let Some(parameters) = route.match_under_its_base_paths(path, method) else {
                continue;
            };
            let better = match &best {
                None => true,
                // The specification's rule that a concrete segment
                // outranks a templated one.
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
    fn parse(
        template: &str,
        method_bases: BTreeMap<String, Vec<String>>,
        additional_bases: BTreeMap<String, Vec<String>>,
        undescribed_bases: Vec<String>,
    ) -> Self {
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
            method_bases: method_bases
                .into_iter()
                .map(|(key, bases)| (key, tidy(bases)))
                .collect(),
            additional_bases: additional_bases
                .into_iter()
                .map(|(key, bases)| (key, tidy(bases)))
                .collect(),
            undescribed_bases: tidy(undescribed_bases),
        }
    }

    /// The base paths that apply to one method — this operation's own,
    /// or every prefix the route answers under when the Path Item
    /// Object does not describe the method at all.
    fn bases_for(&self, method: &str) -> &[String] {
        crate::method::standard(method)
            .and_then(|key| self.method_bases.get(&key))
            .or_else(|| self.additional_bases.get(method))
            .map_or(self.undescribed_bases.as_slice(), Vec::as_slice)
    }

    /// Match `path` with each base path for `method` stripped, longest
    /// first, and then with none stripped.
    fn match_under_its_base_paths(
        &self,
        path: &str,
        method: &str,
    ) -> Option<BTreeMap<String, String>> {
        self.bases_for(method)
            .iter()
            .filter_map(|base| strip_base(path, base))
            .chain(std::iter::once(path))
            .find_map(|candidate| self.match_path(candidate))
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
                    // A literal segment is compared decoded — `%20` and a
                    // space are the same segment. Only captured values
                    // stay encoded, because only they get split later.
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
                // Kept as it arrived; `style` splits it before anything
                // decodes it.
                parameters.insert(name.clone(), value.to_owned());
            }
        }
        index += 1;
    }
    rest.is_empty().then_some(())
}

/// The base paths for each operation of one operation map, keyed as
/// that map keys them.
fn base_paths_per_operation(
    operations: Option<&BTreeMap<String, Operation>>,
    inherited: Option<&[Server]>,
) -> BTreeMap<String, Vec<String>> {
    operations
        .into_iter()
        .flatten()
        .map(|(key, operation)| {
            let servers = operation.servers.as_deref().or(inherited);
            (key.clone(), base_paths_of(servers))
        })
        .collect()
}

/// The base paths a set of Server Objects describes.
fn base_paths_of(servers: Option<&[Server]>) -> Vec<String> {
    servers
        .unwrap_or_default()
        .iter()
        .filter_map(server_base_path)
        .collect()
}

/// Longest first, no empties, no duplicates — the order a prefix is
/// stripped in.
fn tidy(mut bases: Vec<String>) -> Vec<String> {
    bases.retain(|base| !base.is_empty());
    bases.sort_by_key(|base| std::cmp::Reverse(base.len()));
    bases.dedup();
    bases
}

/// The path component of a Server Object's URL/// The path component of a Server Object's URL, with any server
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

    /// A router over exactly these paths and servers.
    fn router_of(
        paths: Vec<(String, PathItem)>,
        servers: Option<Vec<Server>>,
        base_path: Option<&str>,
    ) -> Router {
        let paths: BTreeMap<String, PathItem> = paths.into_iter().collect();
        Router::new(&paths, servers.as_deref(), base_path)
    }

    fn router(templates: &[&str]) -> Router {
        let paths = templates
            .iter()
            .map(|template| ((*template).to_owned(), PathItem::default()))
            .collect();
        router_of(paths, None, None)
    }

    /// Every test below asks about `GET` unless it says otherwise.
    fn matched<'r>(router: &'r Router, path: &str) -> (&'r str, Vec<(String, String)>) {
        matched_with(router, path, "GET")
    }

    fn matched_with<'r>(
        router: &'r Router,
        path: &str,
        method: &str,
    ) -> (&'r str, Vec<(String, String)>) {
        let found = router.route(path, method).expect("path must match");
        (found.template, found.parameters.into_iter().collect())
    }

    fn routes(router: &Router, path: &str) -> bool {
        router.route(path, "GET").is_some()
    }

    #[test]
    fn a_literal_path_matches_itself() {
        let router = router(&["/pets"]);
        assert_eq!(matched(&router, "/pets"), ("/pets", vec![]));
        assert!(!routes(&router, "/pet"));
        assert!(!routes(&router, "/pets/7"));
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
        assert!(!routes(&router, "/report."));
    }

    #[test]
    fn two_adjacent_parameters_cannot_be_told_apart_so_nothing_matches() {
        let router = router(&["/{a}{b}"]);
        assert!(!routes(&router, "/xy"));
    }

    #[test]
    fn a_path_parameter_is_captured_exactly_as_it_arrived() {
        // Not decoded here: `style` splits the value first, and a
        // percent-encoded delimiter is data rather than a separator.
        let router = router(&["/pets/{name}"]);
        assert_eq!(
            matched(&router, "/pets/rex%20the%20dog").1,
            vec![("name".to_owned(), "rex%20the%20dog".to_owned())],
        );
    }

    #[test]
    fn an_encoded_slash_stays_inside_one_parameter() {
        let router = router(&["/files/{path}"]);
        assert_eq!(
            matched(&router, "/files/a%2Fb").1,
            vec![("path".to_owned(), "a%2Fb".to_owned())],
        );
    }

    #[test]
    fn a_literal_segment_is_compared_decoded() {
        let router = router(&["/my pets"]);
        assert_eq!(matched(&router, "/my%20pets").0, "/my pets");
    }

    #[test]
    fn an_empty_parameter_is_not_a_value() {
        let router = router(&["/pets/{petId}"]);
        assert!(!routes(&router, "/pets/"));
    }

    #[test]
    fn a_server_base_path_is_stripped_before_matching() {
        let router = router_of(
            vec![("/pets".to_owned(), PathItem::default())],
            Some(vec![server("https://api.example.com/v1")]),
            None,
        );
        assert_eq!(matched(&router, "/v1/pets").0, "/pets");
        // A proxy may already have stripped it.
        assert_eq!(matched(&router, "/pets").0, "/pets");
    }

    #[test]
    fn a_base_path_only_strips_whole_segments() {
        let router = router_of(
            vec![("/pets".to_owned(), PathItem::default())],
            Some(vec![server("/v1")]),
            None,
        );
        assert!(!routes(&router, "/v10/pets"));
    }

    #[test]
    fn an_explicit_base_path_overrides_the_servers() {
        let router = router_of(
            vec![("/pets".to_owned(), PathItem::default())],
            Some(vec![server("https://api.example.com/v1")]),
            Some("api"),
        );
        assert_eq!(matched(&router, "/api/pets").0, "/pets");
        assert!(!routes(&router, "/v1/pets"));
    }

    #[test]
    fn a_server_variable_is_replaced_by_its_default() {
        let variables = serde_json::from_value(serde_json::json!({
            "version": { "default": "v2", "enum": ["v1", "v2"] }
        }))
        .expect("server variables must parse");
        let router = router_of(
            vec![("/pets".to_owned(), PathItem::default())],
            Some(vec![Server {
                url: "https://api.example.com/{version}".to_owned(),
                variables: Some(variables),
                ..Server::default()
            }]),
            None,
        );
        assert_eq!(matched(&router, "/v2/pets").0, "/pets");
    }

    #[test]
    fn a_server_with_a_variable_and_no_definition_contributes_no_base_path() {
        let router = router_of(
            vec![("/pets".to_owned(), PathItem::default())],
            Some(vec![server("https://api.example.com/{version}")]),
            None,
        );
        assert_eq!(matched(&router, "/pets").0, "/pets");
        assert!(!routes(&router, "/v1/pets"));
    }

    #[test]
    fn a_server_without_a_path_contributes_no_prefix() {
        let router = router_of(
            vec![("/pets".to_owned(), PathItem::default())],
            Some(vec![server("https://api.example.com")]),
            None,
        );
        assert_eq!(matched(&router, "/pets").0, "/pets");
    }

    fn path_item_with(
        servers: Option<Vec<Server>>,
        operation_servers: Option<Vec<Server>>,
    ) -> PathItem {
        let operation = roas::v3_2::operation::Operation {
            servers: operation_servers,
            ..roas::v3_2::operation::Operation::default()
        };
        PathItem {
            servers,
            operations: Some([("get".to_owned(), operation)].into_iter().collect()),
            ..PathItem::default()
        }
    }

    fn server(url: &str) -> Server {
        Server {
            url: url.to_owned(),
            ..Server::default()
        }
    }

    #[test]
    fn a_path_item_server_overrides_the_root_server() {
        let router = router_of(
            vec![(
                "/pets".to_owned(),
                path_item_with(Some(vec![server("https://api.example.com/v2")]), None),
            )],
            Some(vec![server("https://api.example.com/v1")]),
            None,
        );
        assert_eq!(matched(&router, "/v2/pets").0, "/pets");
        assert!(!routes(&router, "/v1/pets"));
    }

    #[test]
    fn an_operation_server_overrides_the_path_item_and_the_root() {
        let router = router_of(
            vec![(
                "/pets".to_owned(),
                path_item_with(
                    Some(vec![server("/v2")]),
                    Some(vec![server("https://api.example.com/v3")]),
                ),
            )],
            Some(vec![server("/v1")]),
            None,
        );
        assert_eq!(matched(&router, "/v3/pets").0, "/pets");
        assert!(!routes(&router, "/v2/pets"));
        assert!(!routes(&router, "/v1/pets"));
    }

    #[test]
    fn one_description_can_serve_two_paths_under_different_bases() {
        let router = router_of(
            vec![
                (
                    "/pets".to_owned(),
                    path_item_with(Some(vec![server("/v1")]), None),
                ),
                (
                    "/orders".to_owned(),
                    path_item_with(Some(vec![server("/v2")]), None),
                ),
            ],
            None,
            None,
        );
        assert_eq!(matched(&router, "/v1/pets").0, "/pets");
        assert_eq!(matched(&router, "/v2/orders").0, "/orders");
        assert!(!routes(&router, "/v2/pets"));
    }

    #[test]
    fn a_path_item_without_operations_still_takes_the_servers_above_it() {
        let router = router_of(
            vec![("/pets".to_owned(), PathItem::default())],
            Some(vec![server("/v1")]),
            None,
        );
        assert_eq!(matched(&router, "/v1/pets").0, "/pets");
    }

    #[test]
    fn an_additional_operations_server_is_taken_into_account_too() {
        let operation = roas::v3_2::operation::Operation {
            servers: Some(vec![server("/v9")]),
            ..roas::v3_2::operation::Operation::default()
        };
        let path_item = PathItem {
            additional_operations: Some([("COPY".to_owned(), operation)].into_iter().collect()),
            ..PathItem::default()
        };
        let router = router_of(vec![("/pets".to_owned(), path_item)], None, None);
        assert_eq!(matched_with(&router, "/v9/pets", "COPY").0, "/pets");
    }

    #[test]
    fn a_router_over_no_paths_matches_nothing() {
        assert!(!routes(&router(&[]), "/pets"));
    }

    #[test]
    fn an_unclosed_brace_is_a_literal() {
        let router = router(&["/pets/{petId"]);
        assert_eq!(matched(&router, "/pets/{petId").0, "/pets/{petId");
    }
}
