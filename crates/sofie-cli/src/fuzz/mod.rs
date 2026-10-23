mod report;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Duration;

use sofie_core::encoding::percent;
use sofie_model::Method;
use sofie_model::naming::path_parameters;
use sofie_values::Seed;
use sofie_values::fixture::field_json;

pub use report::{Finding, FuzzReport, Outcome};

use crate::gen_command::GenError;
use crate::http_client::{Answer, CookieJars, HttpError, Request};
use crate::inputs::{Case, clock_seed, fuzz_case, fuzz_query, valid_case};
use crate::local_app::{LocalApp, wait_for_trees};
use crate::targets::{Target, Targets, member_areas, targets};

const DEFAULT_REQUESTS: NonZeroUsize = NonZeroUsize::new(300).expect("300 is not zero");
const LOG_LINES: usize = 40;
const SHOWN_LOG_LINES: usize = 20;
/// A dropped connection is told apart from a crash once the process had time to exit.
const EXIT_GRACE: Duration = Duration::from_millis(200);
const MALFORMED_IDS: [&str; 5] = [
    "not-an-id",
    "0",
    "-1",
    "%2e%2e%2fadmin",
    "00000000-0000-0000-0000-000000000000x",
];
const MEMBERS: [&str; 2] = ["a", "b"];
const FOREIGN_RECORDS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuzzOptions {
    pub seed: u64,
    /// A run sending nothing would report an app with no findings it never tested.
    pub requests: NonZeroUsize,
}

impl FuzzOptions {
    /// Without a seed each run draws another, and the report names it.
    #[must_use]
    pub fn new(seed: Option<u64>, requests: Option<NonZeroUsize>) -> Self {
        Self {
            seed: seed.unwrap_or_else(clock_seed),
            requests: requests.unwrap_or(DEFAULT_REQUESTS),
        }
    }
}

/// The routes take turns; what each request sends is drawn from the seed,
/// so a run sends the same requests again, apart from the record ids the
/// database lists.
pub fn fuzz(root: &Path, options: FuzzOptions) -> Result<FuzzReport, GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    let Targets { targets, skipped } = targets(&app)?;
    let mut report = FuzzReport {
        seed: options.seed,
        sent: 0,
        routes: targets.len(),
        statuses: BTreeMap::new(),
        skipped,
        findings: Vec::new(),
    };
    if targets.is_empty() {
        return Ok(report);
    }
    let mut run = Run::start(root, &targets)?;
    run.give_the_other_member_records(&targets, options.seed)?;
    let mut seed = Seed::new(options.seed);
    for index in 0..options.requests.get() {
        let target = &targets[index % targets.len()];
        let planned = Planned::draw(target, &mut seed);
        let (status, finding) = run.send(target, &planned)?;
        if let Some(status) = status {
            *report.statuses.entry(status).or_default() += 1;
        }
        if let Some(finding) = finding {
            report.add(finding);
        }
        report.sent += 1;
    }
    Ok(report)
}

#[derive(Clone, Copy)]
enum IdChoice {
    Own,
    Foreign,
    Random,
    Malformed(&'static str),
}

struct Planned {
    answer: Answer,
    signed_in: bool,
    ids: Vec<(IdChoice, u64)>,
    random_id: String,
    query: String,
    case: Option<Case>,
}

impl Planned {
    fn draw(target: &Target, seed: &mut Seed) -> Self {
        let answer = *seed.pick(&Answer::ALL);
        let signed_in = target.member.is_some() && seed.between(0, 7) != 0;
        let parameters = path_parameters(&target.path).len();
        let ids = (0..parameters)
            .map(|_| {
                let choice = match seed.between(0, 3) {
                    0 => IdChoice::Own,
                    1 => IdChoice::Foreign,
                    2 => IdChoice::Random,
                    _ => IdChoice::Malformed(seed.pick(&MALFORMED_IDS)),
                };
                (choice, seed.draw())
            })
            .collect();
        let random_id = field_json::<sofie_core::Uuid>(seed, &[])
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let (query, case) = if target.method == Method::Get {
            (fuzz_query(seed), None)
        } else {
            (String::new(), Some(fuzz_case(&target.fields, seed)))
        };
        Self {
            answer,
            signed_in,
            ids,
            random_id,
            query,
            case,
        }
    }
}

struct Run {
    local: LocalApp,
    jars: CookieJars,
}

impl Run {
    fn start(root: &Path, targets: &[Target]) -> Result<Self, GenError> {
        let local = LocalApp::start(root, "fuzz")?;
        let mut jars = CookieJars::default();
        for (area, paths) in member_areas(targets) {
            for member in MEMBERS {
                let email = format!("fuzz-{member}-{area}@example.com");
                local.sign_in(&mut jars, &jar(area, member), paths, &email)?;
            }
        }
        Ok(Self { local, jars })
    }

    /// So a foreign id names a record that exists; a refused input only
    /// leaves the random ids.
    fn give_the_other_member_records(&self, targets: &[Target], seed: u64) -> Result<(), GenError> {
        let mut seed = Seed::new(seed.wrapping_add(1));
        for target in targets.iter().filter(|target| target.is_creating()) {
            let cookie = self.jars.cookie_header(&jar(&target.area, MEMBERS[1]));
            for _ in 0..FOREIGN_RECORDS {
                let case = valid_case(&target.fields, &mut seed);
                self.local
                    .send_json("POST", &target.path, cookie.clone(), &case.body)
                    .map_err(|error| {
                        GenError::App(format!("{} for the other member: {error}", target.name))
                    })?;
            }
        }
        Ok(())
    }

    /// The status answered, if any, and the finding it makes.
    fn send(
        &mut self,
        target: &Target,
        planned: &Planned,
    ) -> Result<(Option<u16>, Option<Finding>), GenError> {
        let (path, foreign) = self.filled_path(target, planned);
        let mut headers = vec![("origin", self.local.origin())];
        headers.extend(planned.answer.headers());
        let cookie = planned
            .signed_in
            .then(|| self.jars.cookie_header(&jar(&target.area, MEMBERS[0])))
            .flatten();
        headers.extend(cookie.map(|cookie| ("cookie", cookie)));
        let body = planned.case.as_ref().map_or("", |case| case.body.as_str());
        if let Some(case) = &planned.case {
            headers.push(("content-type", case.content_type.to_owned()));
        }
        let cursor = self.local.log.cursor();
        let request = Request {
            method: target.method.name(),
            path: &path,
            headers,
            body: body.as_bytes(),
        };
        let reaches_foreign = foreign && planned.signed_in;
        let (status, outcome, trace_id) = match self.local.send(&request) {
            Ok(response) => {
                let Some(outcome) = answered(response.status, &response.reason, reaches_foreign)
                else {
                    return Ok((Some(response.status), None));
                };
                (
                    Some(response.status),
                    outcome,
                    response.trace_id().map(str::to_owned),
                )
            }
            // Never sent, so it says nothing of the app: the fuzzer drew what it cannot send.
            Err(HttpError::Refused(reason)) => {
                return Err(GenError::App(format!(
                    "sofie fuzz drew a request to {} it cannot send: {reason}",
                    target.name
                )));
            }
            Err(error) => (None, self.failed(&error)?, None),
        };
        let trees = trace_id
            .as_deref()
            .map(|trace_id| wait_for_trees(&self.local.log, trace_id))
            .unwrap_or_default();
        let finding = Finding {
            route: target.name.clone(),
            method: target.method.name(),
            path,
            answer: planned.answer.name(),
            signed_in: planned.signed_in,
            input: planned
                .case
                .as_ref()
                .map(|case| (case.label.clone(), case.body.clone())),
            outcome,
            trace_id,
            trees,
            log: self.log_lines(cursor),
            repeats: 0,
        };
        Ok((status, Some(finding)))
    }

    /// A panicking handler drops the connection; a crash ends the process,
    /// which is started again so the run goes on.
    fn failed(&mut self, error: &HttpError) -> Result<Outcome, GenError> {
        std::thread::sleep(EXIT_GRACE);
        let Some(status) = self.local.app.exited()? else {
            return Ok(Outcome::NoAnswer(error.to_string()));
        };
        let outcome = Outcome::Exited(status.to_string());
        self.local.restart()?;
        Ok(outcome)
    }

    /// The path, and whether it names a record of the other member's.
    fn filled_path(&self, target: &Target, planned: &Planned) -> (String, bool) {
        let mut path = target.path.clone();
        let mut foreign = false;
        for (parameter, (choice, pick)) in
            path_parameters(&target.path).into_iter().zip(&planned.ids)
        {
            let marker = format!("{{{parameter}}}");
            let collection = path[..path.find(&marker).unwrap_or(0)]
                .trim_end_matches('/')
                .to_owned();
            let id = match choice {
                IdChoice::Malformed(id) => (*id).to_owned(),
                IdChoice::Random => planned.random_id.clone(),
                IdChoice::Own | IdChoice::Foreign => {
                    let own = matches!(choice, IdChoice::Own);
                    match pick_id(&self.listed_ids(target, &collection, own), *pick) {
                        Some(id) => {
                            foreign |= !own;
                            path_segment(&id)
                        }
                        None => planned.random_id.clone(),
                    }
                }
            };
            path = path.replacen(&marker, &id, 1);
        }
        (format!("{path}{}", planned.query), foreign)
    }

    /// The other member's records are foreign: what this member must not reach.
    fn listed_ids(&self, target: &Target, collection: &str, own: bool) -> Vec<String> {
        let own_ids = self.ids_at(collection, &jar(&target.area, MEMBERS[0]));
        if own {
            return own_ids;
        }
        self.ids_at(collection, &jar(&target.area, MEMBERS[1]))
            .into_iter()
            .filter(|id| !own_ids.contains(id))
            .collect()
    }

    fn ids_at(&self, collection: &str, jar: &str) -> Vec<String> {
        let cookie = self.jars.cookie_header(jar);
        let Ok(response) = self.local.send_json("GET", collection, cookie, "") else {
            return Vec::new();
        };
        let rows: serde_json::Value = serde_json::from_slice(&response.body).unwrap_or_default();
        listed_ids(&rows)
    }

    fn log_lines(&self, cursor: u64) -> Vec<String> {
        self.local
            .log
            .after(cursor, None, LOG_LINES)
            .lines
            .into_iter()
            .filter_map(|line| line.split_once(' ').map(|(_, text)| text.to_owned()))
            .filter(|text| !text.starts_with(' ') && !text.contains(" sofie.dev: trace_id="))
            .take(SHOWN_LOG_LINES)
            .collect()
    }
}

/// A server error, or a success that let one member reach another's record, is a finding.
fn answered(status: u16, reason: &str, reaches_foreign: bool) -> Option<Outcome> {
    if status >= 500 {
        return Some(Outcome::Status(status, reason.to_owned()));
    }
    let success = (200..300).contains(&status);
    (success && reaches_foreign).then(|| Outcome::ForeignRecord(status, reason.to_owned()))
}

fn listed_ids(rows: &serde_json::Value) -> Vec<String> {
    rows.as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| match &row["id"] {
            serde_json::Value::String(id) => Some(id.clone()),
            serde_json::Value::Number(id) => Some(id.to_string()),
            _ => None,
        })
        .collect()
}

/// An id the app listed reaches the route as it is, whatever it holds.
fn path_segment(id: &str) -> String {
    percent::encode(id, percent::is_unreserved)
}

fn jar(area: &str, member: &str) -> String {
    format!("{area}/{member}")
}

fn pick_id(ids: &[String], pick: u64) -> Option<String> {
    let count = u64::try_from(ids.len()).ok().filter(|count| *count > 0)?;
    let index = usize::try_from(pick % count).ok()?;
    ids.get(index).cloned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Outcome, answered, listed_ids, path_segment};

    #[test]
    fn a_success_is_a_finding_only_when_it_reached_another_member_s_record() {
        assert_eq!(
            answered(200, "OK", true),
            Some(Outcome::ForeignRecord(200, "OK".to_owned()))
        );
        assert_eq!(answered(200, "OK", false), None);
        assert_eq!(answered(404, "Not Found", true), None);
        assert_eq!(
            answered(500, "Internal Server Error", false),
            Some(Outcome::Status(500, "Internal Server Error".to_owned()))
        );
    }

    #[test]
    fn a_listed_id_is_a_string_or_a_number() {
        let rows = json!([{ "id": "a1" }, { "id": 42 }, { "id": null }, { "name": "x" }]);

        assert_eq!(listed_ids(&rows), ["a1", "42"]);
    }

    #[test]
    fn a_listed_id_is_sent_as_one_path_segment() {
        assert_eq!(path_segment("a b/ç"), "a%20b%2F%C3%A7");
        assert_eq!(
            path_segment("0190a6c4-7b8e-7000-8000-000000000000"),
            "0190a6c4-7b8e-7000-8000-000000000000"
        );
    }
}
