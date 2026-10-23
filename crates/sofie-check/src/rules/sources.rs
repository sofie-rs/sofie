use sofie_model::{App, Source};

use super::{Finding, clock, database, environment, outgoing};

pub(super) fn check_sources(app: &App) -> Vec<Finding> {
    let mut findings: Vec<Finding> = app
        .modules
        .iter()
        .flat_map(|module| std::iter::once(&module.krate).chain(&module.contract))
        .flat_map(|krate| &krate.sources)
        .flat_map(check_source)
        .collect();
    findings.extend(app.app.sources.iter().flat_map(app_connections));
    findings
}

/// The app wires the gateways, so it may name `Outgoing`; it opens no connection itself either.
fn app_connections(source: &Source) -> impl Iterator<Item = Finding> + '_ {
    source
        .paths
        .iter()
        .filter(|path| outgoing::opens_connection(&path.segments))
        .map(|path| outgoing::finding(&source.file, path.line, &path.segments.join("::")))
}

fn check_source(source: &Source) -> Vec<Finding> {
    let file = &source.file;
    let mut findings = Vec::new();
    for path in &source.paths {
        let written = path.segments.join("::");
        if environment::path_reads_environment(&path.segments) {
            findings.push(environment::finding(file, path.line, &written));
        }
        if outgoing::opens_connection(&path.segments) {
            findings.push(outgoing::finding(file, path.line, &written));
        }
        if outgoing::holds_outgoing_outside_gateways(file, &path.segments) {
            findings.push(outgoing::outside_gateways_finding(file, path.line));
        }
        if database::names_driver(&path.segments) {
            findings.push(database::finding(file, path.line, &written));
        }
        if clock::reads_system_time(&path.segments) {
            findings.push(clock::finding(file, path.line, &written));
        }
    }
    for call in &source.macros {
        if environment::macro_reads_environment(call) {
            let written = format!("{}!", call.name());
            findings.push(environment::finding(file, call.line, &written));
        }
        if database::names_driver(&call.path) {
            let written = format!("{}!", call.path.join("::"));
            findings.push(database::finding(file, call.line, &written));
        }
    }
    findings
}
