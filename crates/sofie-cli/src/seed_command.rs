use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde_json::Value;
use sofie_model::App;
use sofie_model::naming::file_stem;
use sofie_values::Seed;

use crate::gen_command::GenError;
use crate::http_client::CookieJars;
use crate::inputs::{clock_seed, valid_case};
use crate::local_app::{LocalApp, bench_member};
use crate::source_edits::{files_with_extension, read};
use crate::targets::{Target, Targets, member_areas, targets};

const SEEDS: &str = "seeds";
const SHOWN_REFUSAL: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedReport {
    pub routes: Vec<SeededRoute>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeededRoute {
    pub route: String,
    pub created: usize,
    pub refused: usize,
    pub first_refusal: Option<String>,
}

/// Records are made the way the app makes them: posted to its creating
/// routes, whose actions call the factories, as the member `sofie bench` signs
/// in with, so the load reads them. Each module's `seeds/<area>.<handler>.json`
/// is posted first, then `volume` generated inputs per route.
pub fn seed(root: &Path, volume: usize) -> Result<SeedReport, GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    let found = targets(&app)?;
    let files = seed_files(&app, &found)?;
    let creating: Vec<Target> = found
        .targets
        .into_iter()
        .filter(|target| {
            target.not_postable().is_none()
                && (files.contains_key(&target.name) || (volume > 0 && !target.fields.is_empty()))
        })
        .collect();
    let mut report = SeedReport { routes: Vec::new() };
    if creating.is_empty() {
        return Ok(report);
    }
    let local = LocalApp::start(root, "seed")?;
    let jars = sign_in(&local, &creating)?;
    // Not reproducible on purpose: a second run adds records instead of
    // colliding with the first run's unique values.
    let mut seed = Seed::new(clock_seed());
    for target in &creating {
        let mut seeded = SeededRoute {
            route: target.name.clone(),
            created: 0,
            refused: 0,
            first_refusal: None,
        };
        let fixed = files.get(&target.name).map_or(&[][..], Vec::as_slice);
        for body in fixed {
            seeded.post(&local, &jars, target, &body.to_string());
        }
        let generated = if target.fields.is_empty() { 0 } else { volume };
        for _ in 0..generated {
            let case = valid_case(&target.fields, &mut seed);
            seeded.post(&local, &jars, target, &case.body);
        }
        report.routes.push(seeded);
    }
    Ok(report)
}

fn sign_in(local: &LocalApp, creating: &[Target]) -> Result<CookieJars, GenError> {
    let mut jars = CookieJars::default();
    for (area, paths) in member_areas(creating) {
        local.sign_in(&mut jars, area, paths, &bench_member(area))?;
    }
    Ok(jars)
}

impl SeededRoute {
    fn post(&mut self, local: &LocalApp, jars: &CookieJars, target: &Target, body: &str) {
        let cookie = jars.cookie_header(&target.area);
        let refusal = match local.send_json("POST", &target.path, cookie, body) {
            Ok(response) if response.status < 400 => {
                self.created += 1;
                return;
            }
            Ok(response) => format!(
                "{}: {}",
                response.status,
                String::from_utf8_lossy(&response.body)
            ),
            Err(error) => error.to_string(),
        };
        self.refused += 1;
        if self.first_refusal.is_none() {
            let end = refusal.floor_char_boundary(SHOWN_REFUSAL);
            self.first_refusal = Some(format!("{} for {body}", &refusal[..end]));
        }
    }
}

/// `seeds/<area>.<handler>.json` beside a module's `src/`: a JSON array of
/// inputs to that route.
fn seed_files(app: &App, found: &Targets) -> Result<BTreeMap<String, Vec<Value>>, GenError> {
    let mut files = BTreeMap::new();
    for module in &app.modules {
        let directory = module.krate.path.join(SEEDS);
        for file in files_with_extension(&directory, "json")? {
            let route = format!("{}.{}", module.krate.crate_name, file_stem(&file));
            if let Some(reason) = unseedable(found, &route) {
                return Err(GenError::App(format!(
                    "{}: cannot seed `{route}`: {reason}; a seed file is named \
                     `<area>.<handler>.json` after a POST route without path parameters",
                    file.display()
                )));
            }
            let text = read(&file)?;
            let inputs: Vec<Value> = serde_json::from_str(&text).map_err(|error| {
                GenError::App(format!(
                    "{}: not a JSON array of inputs: {error}",
                    file.display()
                ))
            })?;
            files.insert(route, inputs);
        }
    }
    Ok(files)
}

/// The first reason a seed file's route cannot take its inputs.
fn unseedable(found: &Targets, route: &str) -> Option<String> {
    if let Some(target) = found.targets.iter().find(|target| target.name == route) {
        return target.not_postable().map(str::to_owned);
    }
    if let Some(skipped) = found.skipped.iter().find(|skipped| {
        skipped
            .strip_prefix(route)
            .is_some_and(|rest| rest.starts_with(':'))
    }) {
        return Some(skipped[route.len() + 1..].trim().to_owned());
    }
    Some("no route has that name (`sofie routes` lists them)".to_owned())
}

/// Sorted; none when the directory does not exist.
impl fmt::Display for SeedReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.routes.is_empty() {
            return writeln!(
                formatter,
                "sofie seed: nothing to seed; no POST route without a path parameter takes an \
                 input, or there is no volume and no seed file"
            );
        }
        for route in &self.routes {
            write!(formatter, "{}: {} created", route.route, route.created)?;
            if route.refused > 0 {
                write!(formatter, ", {} refused", route.refused)?;
            }
            writeln!(formatter)?;
            if let Some(refusal) = &route.first_refusal {
                writeln!(formatter, "  first refusal: {refusal}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sofie_model::Method;

    use super::*;

    fn target(name: &str, method: Method, path: &str) -> Target {
        Target {
            name: name.to_owned(),
            module: "exams".to_owned(),
            method,
            path: path.to_owned(),
            area: "web".to_owned(),
            member: None,
            fields: Vec::new(),
        }
    }

    #[test]
    fn a_seed_file_names_why_its_route_takes_no_inputs() {
        let found = Targets {
            targets: vec![
                target("exams.web.add_question", Method::Post, "/questions"),
                target("exams.web.questions", Method::Get, "/questions"),
                target("exams.web.answer", Method::Post, "/questions/{id}/answers"),
            ],
            skipped: vec!["exams.admin.add: area `admin` has no literal prefix".to_owned()],
        };

        assert_eq!(unseedable(&found, "exams.web.add_question"), None);
        assert_eq!(
            unseedable(&found, "exams.web.questions").as_deref(),
            Some("it is not a POST route")
        );
        assert_eq!(
            unseedable(&found, "exams.web.answer").as_deref(),
            Some("its path takes a parameter")
        );
        assert_eq!(
            unseedable(&found, "exams.admin.add").as_deref(),
            Some("area `admin` has no literal prefix")
        );
        assert!(
            unseedable(&found, "exams.web.missing")
                .is_some_and(|reason| reason.contains("no route"))
        );
    }
}
