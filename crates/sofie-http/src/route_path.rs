use std::fmt;

pub use http::Method;
use sofie_core::encoding::percent;
use sofie_db::Aggregate;

/// Made by a contract's route builders, so links and tests name a route by its handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutePath {
    method: Method,
    path: String,
}

#[derive(Clone, Copy, Debug)]
pub struct RouteName {
    pub method: &'static str,
    pub path: &'static str,
    pub name: &'static str,
}

impl RoutePath {
    #[must_use]
    pub fn new(method: Method, path: String) -> Self {
        Self { method, path }
    }

    /// A parameter of the query a GET reads as `Query<Q>` or `Listing<T, Q>`.
    #[must_use]
    pub fn with_query(mut self, name: &str, value: &impl fmt::Display) -> Self {
        let separator = if self.path.contains('?') { '&' } else { '?' };
        self.path.push(separator);
        percent::push_encoded(&mut self.path, name, percent::is_unreserved);
        self.path.push('=');
        percent::push_encoded(&mut self.path, &value.to_string(), percent::is_unreserved);
        self
    }

    #[must_use]
    pub fn method(&self) -> &Method {
        &self.method
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl fmt::Display for RoutePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.path)
    }
}

/// A `/` in `value` stays inside the segment.
#[must_use]
pub fn path_segment(value: &impl fmt::Display) -> String {
    percent::encode(&value.to_string(), percent::is_unreserved)
}

/// For `{*rest}`: each part is encoded, the slashes between them kept.
#[must_use]
pub fn path_rest(value: &str) -> String {
    value
        .split('/')
        .map(|part| percent::encode(part, percent::is_unreserved))
        .collect::<Vec<_>>()
        .join("/")
}

/// Called in a `const` block by a route builder, whose contract cannot name the
/// module's aggregate: an id of any other aggregate fails the build.
pub const fn require_aggregate<A: Aggregate>(module: &str, name: &str) {
    assert!(
        same_text(A::MODULE, module) && same_text(A::NAME, name),
        "a route builder takes the id of the aggregate its path parameter is named after"
    );
}

pub(crate) const fn same_text(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{Method, RoutePath, path_rest, path_segment};

    #[test]
    fn a_query_parameter_is_encoded_after_the_path() {
        let path = RoutePath::new(Method::GET, "/shops".to_owned())
            .with_query("near", &"Ulm Mitte")
            .with_query("radius", &5);

        assert_eq!(path.path(), "/shops?near=Ulm%20Mitte&radius=5");
    }

    #[test]
    fn a_segment_keeps_its_slashes_and_spaces_inside_it() {
        assert_eq!(path_segment(&"a/b c"), "a%2Fb%20c");
    }

    #[test]
    fn the_rest_of_a_path_keeps_its_slashes() {
        assert_eq!(path_rest("a b/c?d"), "a%20b/c%3Fd");
    }
}
