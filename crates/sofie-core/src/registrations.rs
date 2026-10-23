//! Run by both `sofie check` and boot, so the two cannot drift.

use std::ops::RangeInclusive;
use std::time::Duration;

/// Faster claims without pause and never lets a drain end; slower overflows the next due time.
const INTERVALS: RangeInclusive<Duration> =
    Duration::from_secs(1)..=Duration::from_hours(1000 * 365 * 24);

pub const INTERVALS_ALLOWED: &str =
    "a schedule runs at most once a second and at least once in 1000 years";

#[derive(Clone, Copy, Debug)]
pub enum Key<'a> {
    /// Prefix included; `sofie check` knows no path for an area without a literal prefix.
    Route {
        method: &'a str,
        path: Option<&'a str>,
        name: Option<&'a str>,
    },
    Job {
        name: &'a str,
        every: Option<Duration>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    SamePath(usize, usize),
    SameRouteName(usize, usize),
    SameJobName(usize, usize),
    Interval(usize),
}

impl Problem {
    #[must_use]
    pub fn pair(self) -> Option<(usize, usize)> {
        match self {
            Self::SamePath(first, second)
            | Self::SameRouteName(first, second)
            | Self::SameJobName(first, second) => Some((first, second)),
            Self::Interval(_) => None,
        }
    }
}

#[must_use]
pub fn validate(keys: &[Key<'_>]) -> Vec<Problem> {
    let mut problems: Vec<Problem> = keys
        .iter()
        .enumerate()
        .filter(|(_, key)| runs_out_of_range(key))
        .map(|(index, _)| Problem::Interval(index))
        .collect();
    let served: Vec<Option<String>> = keys.iter().map(served_at).collect();
    let pairs = (0..keys.len()).flat_map(|second| (0..second).map(move |first| (first, second)));
    for (first, second) in pairs {
        match (keys[first], keys[second]) {
            (
                Key::Route { method, name, .. },
                Key::Route {
                    method: other_method,
                    name: other_name,
                    ..
                },
            ) => {
                if method == other_method
                    && routers_clash(served[first].as_deref(), served[second].as_deref())
                {
                    problems.push(Problem::SamePath(first, second));
                }
                if name.is_some() && name == other_name {
                    problems.push(Problem::SameRouteName(first, second));
                }
            }
            (Key::Job { name, .. }, Key::Job { name: other, .. }) if name == other => {
                problems.push(Problem::SameJobName(first, second));
            }
            _ => {}
        }
    }
    problems
}

fn runs_out_of_range(key: &Key<'_>) -> bool {
    matches!(key, Key::Job { every: Some(every), .. } if !INTERVALS.contains(every))
}

fn served_at(key: &Key<'_>) -> Option<String> {
    match key {
        Key::Route { path, .. } => path.map(without_parameter_names),
        Key::Job { .. } => None,
    }
}

/// As the router refuses them: one path once parameters are erased, or a catch-all at a parameter.
fn routers_clash(left: Option<&str>, right: Option<&str>) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    let common = left
        .bytes()
        .zip(right.bytes())
        .take_while(|(left, right)| left == right)
        .count();
    left == right || left.as_bytes()[..common].ends_with(b"{")
}

fn without_parameter_names(path: &str) -> String {
    path.split('/')
        .map(|segment| match segment.strip_prefix('{') {
            Some(parameter) if parameter.starts_with('*') => "{*}",
            Some(_) => "{}",
            None => segment,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Key, Problem, validate};

    fn route<'a>(method: &'a str, path: &'a str, name: &'a str) -> Key<'a> {
        Key::Route {
            method,
            path: Some(path),
            name: Some(name),
        }
    }

    fn job(name: &str, every: Option<Duration>) -> Key<'_> {
        Key::Job { name, every }
    }

    #[test]
    fn distinct_routes_and_jobs_pass() {
        let keys = [
            route("GET", "/orders/{order}", "order.web.show"),
            route("POST", "/orders/{order}", "order.web.cancel"),
            route("GET", "/orders/new", "order.web.new"),
            job("order.listeners.audit", None),
            job("order.schedules.tidy", Some(Duration::from_mins(15))),
        ];
        assert_eq!(validate(&keys), []);
    }

    #[test]
    fn paths_alike_but_for_parameter_names_clash() {
        let keys = [
            route("GET", "/account/orders/{order}", "order.customer.show"),
            route("GET", "/account/orders/{id}", "order.web.show"),
        ];
        assert_eq!(validate(&keys), [Problem::SamePath(0, 1)]);
    }

    #[test]
    fn a_parameter_and_a_catch_all_at_one_place_clash() {
        let keys = [
            route("GET", "/files/{file}", "storage.web.show"),
            route("GET", "/files/{*rest}", "storage.web.serve"),
        ];
        assert_eq!(validate(&keys), [Problem::SamePath(0, 1)]);
    }

    #[test]
    fn a_route_name_is_one_route_s() {
        let keys = [
            route("GET", "/orders", "order.web.list"),
            route("GET", "/all-orders", "order.web.list"),
        ];
        assert_eq!(validate(&keys), [Problem::SameRouteName(0, 1)]);
    }

    #[test]
    fn unnamed_routes_never_share_a_name() {
        let unnamed = |path| Key::Route {
            method: "GET",
            path: Some(path),
            name: None,
        };
        assert_eq!(validate(&[unnamed("/a"), unnamed("/b")]), []);
    }

    #[test]
    fn a_route_whose_path_is_unknown_clashes_with_none() {
        let unplaced = Key::Route {
            method: "GET",
            path: None,
            name: Some("order.member.orders"),
        };
        let keys = [unplaced, route("GET", "/orders", "order.web.orders")];
        assert_eq!(validate(&keys), []);
    }

    #[test]
    fn a_job_name_is_one_job_s() {
        let keys = [
            job("tidy", Some(Duration::from_mins(1))),
            route("GET", "/tidy", "tidy"),
            job("tidy", None),
        ];
        assert_eq!(validate(&keys), [Problem::SameJobName(0, 2)]);
    }

    #[test]
    fn a_schedule_runs_between_once_a_second_and_once_in_1000_years() {
        let keys = [
            job("eager", Some(Duration::from_millis(500))),
            job("second", Some(Duration::from_secs(1))),
            job("never", Some(Duration::from_hours(1001 * 365 * 24))),
        ];
        assert_eq!(
            validate(&keys),
            [Problem::Interval(0), Problem::Interval(2)]
        );
    }
}
