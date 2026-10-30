//! Routing as Starlette does it for the FastAPI application: routes are tried in registration
//! order; the first whose path and method match handles the request; when none does but some
//! path matched with another method, the first of those answers 405 with its methods in
//! `Allow`. The catch-all `GET /{path:path}` comes last, so an unknown `GET` reaches the SPA
//! route while any other unknown method is a 405.
//!
//! Paths are matched after percent-decoding, as Starlette matches `scope["path"]`.

use wg_iface::http::text::unquote;

/// One piece of a path template.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    /// `{name}`: one non-empty segment.
    Param(String),
    /// `{name:path}`: the rest of the path, possibly empty or holding slashes.
    Rest(String),
}

/// A path template such as `/api/v1/ws/{ws}/tasks/{ref}` or `/{path:path}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub path: String,
    parts: Vec<Part>,
}

impl Template {
    /// Parses a template; parameters take whole segments.
    pub fn parse(path: &str) -> Template {
        let parts = path
            .split('/')
            .skip(1)
            .map(|segment| {
                match segment
                    .strip_prefix('{')
                    .and_then(|inner| inner.strip_suffix('}'))
                {
                    Some(inner) => match inner.split_once(':') {
                        Some((name, "path")) => Part::Rest(name.to_owned()),
                        Some((name, _)) => Part::Param(name.to_owned()),
                        None => Part::Param(inner.to_owned()),
                    },
                    None => Part::Literal(segment.to_owned()),
                }
            })
            .collect();
        Template {
            path: path.to_owned(),
            parts,
        }
    }

    /// The parameter names, in order.
    pub fn param_names(&self) -> Vec<String> {
        self.parts
            .iter()
            .filter_map(|part| match part {
                Part::Param(name) | Part::Rest(name) => Some(name.clone()),
                Part::Literal(_) => None,
            })
            .collect()
    }

    /// The parameters when `path` (decoded, starting with `/`) matches.
    pub fn matches(&self, path: &str) -> Option<Vec<(String, String)>> {
        let rest = path.strip_prefix('/')?;
        let segments: Vec<&str> = rest.split('/').collect();
        let mut params = Vec::new();
        for (index, part) in self.parts.iter().enumerate() {
            match part {
                Part::Rest(name) => {
                    let tail = segments
                        .get(index..)
                        .filter(|tail| !tail.is_empty())
                        .map(|tail| tail.join("/"))?;
                    params.push((name.clone(), tail));
                    return Some(params);
                }
                Part::Literal(text) => {
                    if segments.get(index) != Some(&text.as_str()) {
                        return None;
                    }
                }
                Part::Param(name) => {
                    let value = segments.get(index).filter(|value| !value.is_empty())?;
                    params.push((name.clone(), (*value).to_owned()));
                }
            }
        }
        (segments.len() == self.parts.len()).then_some(params)
    }
}

/// One route: the methods it answers (`None` for a mount, which answers every method), the
/// `Allow` header of its 405, its template, and what handles it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry<T> {
    pub methods: Option<Vec<String>>,
    pub allow: String,
    pub template: Template,
    pub target: T,
}

impl<T> Entry<T> {
    /// A FastAPI route (`api.get(...)`, `router.post(...)`): exactly one method, no `HEAD`.
    pub fn api(method: &str, path: &str, target: T) -> Entry<T> {
        Entry {
            methods: Some(vec![method.to_owned()]),
            allow: method.to_owned(),
            template: Template::parse(path),
            target,
        }
    }

    /// A plain Starlette `GET` route (the OpenAPI and docs pages): `GET` and `HEAD`.
    pub fn page(path: &str, target: T) -> Entry<T> {
        Entry {
            methods: Some(vec!["GET".to_owned(), "HEAD".to_owned()]),
            allow: "HEAD, GET".to_owned(),
            template: Template::parse(path),
            target,
        }
    }

    /// A mount at `prefix`: every method, every path under `prefix/`.
    pub fn mount(prefix: &str, target: T) -> Entry<T> {
        Entry {
            methods: None,
            allow: String::new(),
            template: Template::parse(&format!("{prefix}/{{path:path}}")),
            target,
        }
    }

    fn answers(&self, method: &str) -> bool {
        self.methods
            .as_ref()
            .is_none_or(|methods| methods.iter().any(|m| m == method))
    }
}

/// What routing found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found<'a, T> {
    Route {
        target: &'a T,
        params: Vec<(String, String)>,
    },
    /// 405 with this `Allow`.
    NotAllowed(String),
    /// No route matched the path: Starlette's plain 404 (unreachable while the catch-all exists).
    Missing,
}

/// Routes `method path` through `entries` in order.
pub fn find<'a, T>(entries: &'a [Entry<T>], method: &str, path: &str) -> Found<'a, T> {
    let mut partial: Option<&Entry<T>> = None;
    for entry in entries {
        if let Some(params) = entry.template.matches(path) {
            if entry.answers(method) {
                return Found::Route {
                    target: &entry.target,
                    params,
                };
            }
            partial = partial.or(Some(entry));
        }
    }
    match partial {
        Some(entry) => Found::NotAllowed(entry.allow.clone()),
        None => Found::Missing,
    }
}

/// `scope["path"]`: the request target's path, percent-decoded.
pub fn decode_path(raw: &str) -> String {
    unquote(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_match_whole_segments() {
        let t = Template::parse("/api/v1/ws/{ws}/tasks/{ref}");
        assert_eq!(
            t.matches("/api/v1/ws/main/tasks/WG-1"),
            Some(vec![
                ("ws".into(), "main".into()),
                ("ref".into(), "WG-1".into())
            ])
        );
        assert_eq!(t.matches("/api/v1/ws/main/tasks/"), None);
        assert_eq!(t.matches("/api/v1/ws/main/tasks/WG-1/x"), None);
        assert_eq!(t.param_names(), vec!["ws", "ref"]);
        let all = Template::parse("/{path:path}");
        assert_eq!(all.matches("/"), Some(vec![("path".into(), "".into())]));
        assert_eq!(
            all.matches("/a/b/"),
            Some(vec![("path".into(), "a/b/".into())])
        );
        let mount = Template::parse("/mcp/{path:path}");
        assert_eq!(mount.matches("/mcp"), None);
        assert!(mount.matches("/mcp/").is_some());
    }

    #[test]
    fn first_full_match_then_first_partial() {
        let entries = vec![
            Entry::page("/api/openapi.json", 0),
            Entry::api("GET", "/api/v1/health", 1),
            Entry::api("GET", "/api/v1/ws/{ws}/tasks/{ref}", 2),
            Entry::api("PATCH", "/api/v1/ws/{ws}/tasks/{ref}", 3),
            Entry::api("POST", "/api/v1/ws/{ws}/stacks/{ref}/integrate", 4),
            Entry::mount("/mcp", 5),
            Entry::api("GET", "/{path:path}", 6),
        ];
        let target = |method: &str, path: &str| match find(&entries, method, path) {
            Found::Route { target, .. } => Ok(*target),
            Found::NotAllowed(allow) => Err(allow),
            Found::Missing => Err("missing".into()),
        };
        assert_eq!(target("POST", "/api/v1/health"), Err("GET".into()));
        assert_eq!(target("HEAD", "/api/v1/health"), Err("GET".into()));
        assert_eq!(target("HEAD", "/api/openapi.json"), Ok(0));
        assert_eq!(target("POST", "/api/openapi.json"), Err("HEAD, GET".into()));
        assert_eq!(
            target("DELETE", "/api/v1/ws/a/tasks/WG-1"),
            Err("GET".into())
        );
        assert_eq!(target("PATCH", "/api/v1/ws/a/tasks/bulk"), Ok(3));
        assert_eq!(target("GET", "/api/v1/ws/a/stacks/X/integrate"), Ok(6));
        assert_eq!(target("POST", "/mcp"), Err("GET".into()));
        assert_eq!(target("DELETE", "/mcp/"), Ok(5));
        assert_eq!(target("GET", "/mcp"), Ok(6));
        assert_eq!(decode_path("/%61pi/v1/nope"), "/api/v1/nope");
    }
}
