use std::path::{Path, PathBuf};
use std::time::Duration;

use sofie_core::registrations::{INTERVALS_ALLOWED, Key, Problem, validate};
use sofie_model::naming::{RouteSite, job_name};
use sofie_model::{App, Cadence, Method, ModelError, Module, ServedRoute, interval_seconds};

use super::{Finding, Rule};

enum Registered {
    Route {
        method: &'static str,
        path: Option<String>,
        name: Option<String>,
    },
    Job {
        name: String,
        every: Option<Duration>,
    },
}

struct Reported {
    file: PathBuf,
    line: usize,
    subject: String,
}

/// `reported` is `None` for the framework packages' routes, which no file of the app declares.
struct Registration {
    registered: Registered,
    reported: Option<Reported>,
    described: String,
}

impl Registration {
    fn key(&self) -> Key<'_> {
        match &self.registered {
            Registered::Route { method, path, name } => Key::Route {
                method,
                path: path.as_deref(),
                name: name.as_deref(),
            },
            Registered::Job { name, every } => Key::Job {
                name,
                every: *every,
            },
        }
    }
}

/// The routes and jobs as boot registers them, held to the rules boot holds them to.
pub(super) fn check_registrations(app: &App) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut registrations: Vec<Registration> = app
        .route_sites()
        .into_iter()
        .map(|site| route_registration(app, site))
        .collect();
    let package_routes = app.package_routes();
    match package_routes.identity() {
        Ok(served) => registrations.extend(served.iter().filter_map(package_registration)),
        Err(error) => findings.push(unread_identity_routes(error)),
    }
    registrations.extend(
        package_routes
            .other_packages()
            .iter()
            .filter_map(package_registration),
    );
    registrations.extend(app.modules.iter().flat_map(job_registrations));
    let keys: Vec<Key> = registrations.iter().map(Registration::key).collect();
    let problems = validate(&keys);
    findings.extend(
        (0..registrations.len()).flat_map(|index| findings_of(&registrations, &problems, index)),
    );
    findings
}

fn route_registration(app: &App, site: RouteSite<'_>) -> Registration {
    let (file, line) = (&site.area.routes_file, site.route.line);
    Registration {
        registered: Registered::Route {
            method: site.route.method.name(),
            path: site.full_path(app),
            name: Some(site.name()),
        },
        reported: Some(Reported {
            file: file.clone(),
            line,
            subject: format!(
                "{} {} in area `{}`",
                site.route.method.name(),
                site.route.path,
                site.area.area
            ),
        }),
        described: format!(
            "module `{}` at {}:{line}",
            site.module.krate.package,
            file.display()
        ),
    }
}

fn package_registration(served: &ServedRoute<'_>) -> Option<Registration> {
    let method = Method::from_routes_method(&served.route.method.to_ascii_lowercase())?;
    Some(Registration {
        registered: Registered::Route {
            method: method.name(),
            path: Some(served.path.clone()),
            name: Some(served.name.clone()),
        },
        reported: None,
        described: format!("{}'s `{}`", served.package(), served.route.handler),
    })
}

/// Named as the wiring registers them with the worker.
fn job_registrations(module: &Module) -> Vec<Registration> {
    let job = |kind: &str, file: &Path, line: usize, function: &[String], every| {
        let name = job_name(module, function);
        Registration {
            described: format!("the {kind} at {}:{line}", file.display()),
            reported: Some(Reported {
                file: file.to_path_buf(),
                line,
                subject: format!("{kind} `{name}`"),
            }),
            registered: Registered::Job { name, every },
        }
    };
    let listeners = module.listeners.iter().map(|listener| {
        job(
            "listener",
            &listener.file,
            listener.line,
            &listener.function,
            None,
        )
    });
    let jobs = module
        .jobs
        .iter()
        .map(|queued| job("job", &queued.file, queued.line, &queued.function, None));
    let schedules = module.schedules.iter().flat_map(|schedule| {
        let every = match &schedule.cadence {
            Cadence::Interval(interval) => interval_seconds(interval).map(Duration::from_secs),
            Cadence::WallClock(_) => None,
        };
        let (file, line) = (&schedule.file, schedule.line);
        match &schedule.each {
            None => vec![job("schedule", file, line, &schedule.function, every)],
            Some(each) => vec![
                job("schedule", file, line, each, every),
                job("job", file, line, &schedule.function, None),
            ],
        }
    });
    listeners.chain(jobs).chain(schedules).collect()
}

fn findings_of(registrations: &[Registration], problems: &[Problem], index: usize) -> Vec<Finding> {
    let Some(reported) = &registrations[index].reported else {
        return Vec::new();
    };
    let described = |kind: fn(usize, usize) -> Problem| -> Vec<&str> {
        others(problems, index, kind)
            .into_iter()
            .map(|other| registrations[other].described.as_str())
            .collect()
    };
    match &registrations[index].registered {
        Registered::Route { path, name, .. } => {
            let clashing = described(Problem::SamePath);
            let named_alike = described(Problem::SameRouteName);
            let conflicts = path
                .as_deref()
                .filter(|_| !clashing.is_empty())
                .map(|path| conflict(reported, path, &clashing));
            let names = name
                .as_deref()
                .filter(|_| !named_alike.is_empty())
                .map(|name| shared_route_name(reported, name, named_alike.len()));
            conflicts.into_iter().chain(names).collect()
        }
        Registered::Job { every, .. } => {
            let jobs_alike = described(Problem::SameJobName);
            let names = (!jobs_alike.is_empty()).then(|| shared_job_name(reported, &jobs_alike));
            let interval = every
                .filter(|_| problems.contains(&Problem::Interval(index)))
                .map(|every| interval_out_of_range(reported, every));
            names.into_iter().chain(interval).collect()
        }
    }
}

fn others(problems: &[Problem], index: usize, kind: fn(usize, usize) -> Problem) -> Vec<usize> {
    problems
        .iter()
        .filter_map(|problem| {
            let (first, second) = problem.pair()?;
            (kind(first, second) == *problem).then_some((first, second))
        })
        .filter_map(|(first, second)| match index {
            _ if index == first => Some(second),
            _ if index == second => Some(first),
            _ => None,
        })
        .collect()
}

fn finding(rule: Rule, reported: &Reported, message: String, fix: &str) -> Finding {
    Finding {
        rule,
        file: reported.file.clone(),
        line: reported.line,
        message,
        fix: fix.to_owned(),
    }
}

fn conflict(reported: &Reported, path: &str, others: &[&str]) -> Finding {
    let message = format!(
        "{} is served at {path}, which is also routed by {}, and only one handler can answer a \
         path",
        reported.subject,
        others.join(", ")
    );
    finding(
        Rule::RouteConflict,
        reported,
        message,
        "give one of these routes a different path or method",
    )
}

fn shared_route_name(reported: &Reported, name: &str, others: usize) -> Finding {
    let message = format!(
        "{} is named `{name}` like {others} other route(s), since a route is named after its \
         handler; spans and route builders need one name per route",
        reported.subject
    );
    finding(
        Rule::RouteName,
        reported,
        message,
        "route each handler once per area; give the other route a handler of its own",
    )
}

fn shared_job_name(reported: &Reported, others: &[&str]) -> Finding {
    let message = format!(
        "{} shares its name with {}, and two jobs of one name would run each other's messages",
        reported.subject,
        others.join(", ")
    );
    finding(
        Rule::JobName,
        reported,
        message,
        "a job is named `<module>.<function path>`; give one of them a function of its own",
    )
}

fn interval_out_of_range(reported: &Reported, every: Duration) -> Finding {
    let message = format!(
        "{} runs every {every:?}; {INTERVALS_ALLOWED}",
        reported.subject
    );
    finding(
        Rule::ScheduleInterval,
        reported,
        message,
        "write an interval from `\"1s\"` up to `\"365000d\"`",
    )
}

fn unread_identity_routes(error: ModelError) -> Finding {
    Finding {
        rule: Rule::RouteConflict,
        file: error.file,
        line: error.line,
        message: format!(
            "sofie-identity's routes could not be read, so their conflicts are unchecked: {}",
            error.message
        ),
        fix: "make the area's file read and parse".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Problem, others};

    #[test]
    fn a_key_s_others_are_every_pair_it_is_in() {
        let problems = [
            Problem::SamePath(0, 2),
            Problem::SameRouteName(0, 1),
            Problem::SamePath(2, 3),
        ];
        assert_eq!(others(&problems, 2, Problem::SamePath), [0, 3]);
        assert_eq!(others(&problems, 1, Problem::SamePath), Vec::<usize>::new());
        assert_eq!(others(&problems, 1, Problem::SameRouteName), [0]);
    }
}
