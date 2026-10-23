use http::Method;
use sofie_core::{Error, Result};

use crate::target::Target;
use crate::urlencoded::decode_path_segment;

#[derive(Default)]
pub(crate) struct Router {
    methods: Vec<(Method, matchit::Router<Target>)>,
}

pub(crate) enum Lookup<'r> {
    Found(&'r Target, Vec<(String, String)>),
    WrongMethod(Vec<&'r str>),
    Missing,
}

impl Router {
    pub(crate) fn insert(&mut self, method: &Method, path: &str, target: Target) -> Result<()> {
        self.tree(method)
            .insert(path, target)
            .map_err(|error| Error::internal(format!("registering route {method} {path}"), error))
    }

    fn tree(&mut self, method: &Method) -> &mut matchit::Router<Target> {
        let index = self.methods.iter().position(|(given, _)| given == method);
        let index = index.unwrap_or_else(|| {
            self.methods.push((method.clone(), matchit::Router::new()));
            self.methods.len() - 1
        });
        &mut self.methods[index].1
    }

    /// `HEAD` falls back to the `GET` tree; the pipeline drops the body.
    pub(crate) fn find(&self, method: &Method, path: &str) -> Lookup<'_> {
        let found = self.found(method, path).or_else(|| {
            (method == Method::HEAD)
                .then(|| self.found(&Method::GET, path))
                .flatten()
        });
        if let Some(found) = found {
            return found;
        }
        let mut allowed: Vec<&str> = self
            .methods
            .iter()
            .filter(|(_, tree)| {
                tree.at(path)
                    .is_ok_and(|found| !is_behind_flag(found.value))
            })
            .map(|(method, _)| method.as_str())
            .collect();
        if allowed.is_empty() {
            return Lookup::Missing;
        }
        if allowed.contains(&"GET") {
            allowed.push("HEAD");
        }
        Lookup::WrongMethod(allowed)
    }

    fn found(&self, method: &Method, path: &str) -> Option<Lookup<'_>> {
        let (_, tree) = self
            .methods
            .iter()
            .find(|(candidate, _)| candidate == method)?;
        let found = tree.at(path).ok()?;
        // A segment that does not decode to UTF-8 names no record, so the path is missing.
        let parameters = found
            .params
            .iter()
            .map(|(name, value)| Some((name.to_owned(), decode_path_segment(value)?)))
            .collect::<Option<_>>();
        Some(parameters.map_or(Lookup::Missing, |parameters| {
            Lookup::Found(found.value, parameters)
        }))
    }
}

/// Its flag is judged per caller once the session is open; until then naming its method in
/// `Allow` would reveal a route its flag hides, so a flagged one is left out for everyone.
#[cfg(feature = "flags")]
fn is_behind_flag(target: &Target) -> bool {
    target.admission.rules.feature.is_some()
}

#[cfg(not(feature = "flags"))]
fn is_behind_flag(_target: &Target) -> bool {
    false
}
