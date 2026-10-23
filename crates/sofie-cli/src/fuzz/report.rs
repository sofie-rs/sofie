use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

const SHOWN_BODY: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Status(u16, String),
    /// A success to a signed-in member asking for another member's record.
    ForeignRecord(u16, String),
    NoAnswer(String),
    Exited(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub route: String,
    pub method: &'static str,
    pub path: String,
    pub answer: &'static str,
    pub signed_in: bool,
    /// The case's label and the body sent.
    pub input: Option<(String, String)>,
    pub outcome: Outcome,
    /// None when the app answered nothing.
    pub trace_id: Option<String>,
    pub trees: Vec<String>,
    pub log: Vec<String>,
    pub repeats: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuzzReport {
    pub seed: u64,
    pub sent: usize,
    pub routes: usize,
    pub statuses: BTreeMap<u16, usize>,
    pub skipped: Vec<String>,
    pub findings: Vec<Finding>,
}

impl Outcome {
    fn kind(&self) -> String {
        match self {
            Self::Status(status, _) => status.to_string(),
            Self::ForeignRecord(..) => "foreign record".to_owned(),
            Self::NoAnswer(_) => "no answer".to_owned(),
            Self::Exited(_) => "exited".to_owned(),
        }
    }
}

impl Finding {
    fn label(&self) -> &str {
        self.input.as_ref().map_or("", |(label, _)| label.as_str())
    }
}

impl FuzzReport {
    /// One finding per route, outcome and kind of input; the others count.
    pub(crate) fn add(&mut self, finding: Finding) {
        let same = self.findings.iter_mut().find(|found| {
            found.route == finding.route
                && found.outcome.kind() == finding.outcome.kind()
                && found.label() == finding.label()
        });
        match same {
            Some(found) => found.repeats += 1,
            None => self.findings.push(finding),
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status, reason) => write!(formatter, "answered {status} {reason}"),
            Self::ForeignRecord(status, reason) => write!(
                formatter,
                "answered {status} {reason} for another member's record"
            ),
            Self::NoAnswer(error) => write!(formatter, "answered nothing ({error})"),
            Self::Exited(status) => write!(formatter, "crashed the app ({status})"),
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let principal = if self.signed_in {
            "signed in"
        } else {
            "signed out"
        };
        writeln!(
            formatter,
            "{} {} ({}) {} as {}, {principal}",
            self.method, self.path, self.route, self.outcome, self.answer
        )?;
        if let Some((label, body)) = &self.input {
            writeln!(formatter, "  input: {label}: {}", excerpt(body))?;
        }
        if self.repeats > 0 {
            writeln!(formatter, "  and {} more like it", self.repeats)?;
        }
        if let Some(trace_id) = &self.trace_id {
            writeln!(formatter, "  trace {trace_id}")?;
        }
        for line in self.trees.iter().flat_map(|tree| tree.lines()) {
            writeln!(formatter, "    {line}")?;
        }
        if !self.log.is_empty() {
            writeln!(formatter, "  log:")?;
        }
        for line in &self.log {
            writeln!(formatter, "    {line}")?;
        }
        Ok(())
    }
}

impl fmt::Display for FuzzReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let findings = match self.findings.len() {
            0 => "no 500, panic or foreign record reached".to_owned(),
            1 => "1 finding".to_owned(),
            count => format!("{count} findings"),
        };
        writeln!(
            formatter,
            "sofie fuzz: {} requests to {} routes, seed {}: {findings}",
            self.sent, self.routes, self.seed
        )?;
        let statuses: Vec<String> = self
            .statuses
            .iter()
            .map(|(status, count)| format!("{status} ×{count}"))
            .collect();
        if !statuses.is_empty() {
            writeln!(formatter, "  answered {}", statuses.join(", "))?;
        }
        for skipped in &self.skipped {
            writeln!(formatter, "  {skipped}")?;
        }
        for (number, finding) in self.findings.iter().enumerate() {
            write!(formatter, "\n{}. {finding}", number + 1)?;
        }
        Ok(())
    }
}

fn excerpt(body: &str) -> String {
    if body.is_empty() {
        return "(empty)".to_owned();
    }
    if body.len() <= SHOWN_BODY {
        return body.to_owned();
    }
    let end = body.floor_char_boundary(SHOWN_BODY);
    let mut shown = body[..end].to_owned();
    let _ = write!(shown, "… ({} bytes)", body.len());
    shown
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Finding, FuzzReport, Outcome};

    fn finding(label: &str, status: u16) -> Finding {
        Finding {
            route: "order.customer.place_order".to_owned(),
            method: "POST",
            path: "/account/orders".to_owned(),
            answer: "json",
            signed_in: true,
            input: Some((
                label.to_owned(),
                format!("{{\"total\":\"{}\"}}", "9".repeat(400)),
            )),
            outcome: Outcome::Status(status, "Internal Server Error".to_owned()),
            trace_id: Some("4bf92f3577b34da6a3ce929d0e0e4736".to_owned()),
            trees: vec!["request POST /account/orders 3.10 ms\n  handler 2.00 ms".to_owned()],
            log: vec!["2026-10-13T09:00:00Z ERROR sofie_http: the handler failed".to_owned()],
            repeats: 0,
        }
    }

    #[test]
    fn a_finding_names_the_route_its_input_status_and_trace() {
        let mut report = FuzzReport {
            seed: 42,
            sent: 300,
            routes: 5,
            statuses: BTreeMap::from([(200, 120), (500, 3)]),
            skipped: vec!["order.admin.list: reached signed out only".to_owned()],
            findings: Vec::new(),
        };
        report.add(finding("oversized total", 500));
        report.add(finding("oversized total", 500));
        report.add(finding("missing total", 500));

        let text = report.to_string();

        assert_eq!(report.findings.len(), 2);
        assert!(
            text.starts_with(
                "sofie fuzz: 300 requests to 5 routes, seed 42: 2 findings\n  \
                 answered 200 ×120, 500 ×3\n  order.admin.list: reached signed out only\n\n1. POST /account/orders \
                 (order.customer.place_order) answered 500 Internal Server Error as json, signed in\n  \
                 input: oversized total: {\"total\":\"999"
            ),
            "{text}"
        );
        assert!(
            text.contains("… (412 bytes)\n  and 1 more like it\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "  trace 4bf92f3577b34da6a3ce929d0e0e4736\n    request POST /account/orders \
                 3.10 ms\n      handler 2.00 ms\n  log:\n    2026-10-13T09:00:00Z ERROR"
            ),
            "{text}"
        );
        assert!(text.contains("\n2. POST /account/orders"), "{text}");
    }

    #[test]
    fn reaching_another_member_s_record_is_its_own_finding() {
        let mut report = FuzzReport {
            seed: 42,
            sent: 2,
            routes: 1,
            statuses: BTreeMap::new(),
            skipped: Vec::new(),
            findings: Vec::new(),
        };
        let mut reached = finding("oversized total", 500);
        reached.outcome = Outcome::ForeignRecord(200, "OK".to_owned());
        report.add(finding("oversized total", 500));
        report.add(reached);

        assert_eq!(report.findings.len(), 2);
        assert!(
            report
                .to_string()
                .contains("answered 200 OK for another member's record as json, signed in"),
            "{report}"
        );
    }
}
