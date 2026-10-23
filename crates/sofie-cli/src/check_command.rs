use std::path::Path;

use sofie_check::{
    AllowList, Date, Finding, MAX_UNUSED_PUBLIC_RATIO, Rule, Screens, check_dead_code,
};
use sofie_model::{App, Crate};

use crate::clients::has_clients;
use crate::gen_command::{GenError, GenMode, GenOutcome, sync_generated};
use crate::translations::AppLanguages;

const DERIVES_PACKAGE: &str = "sofie-macros";

/// Generated files are rendered only from a model the rules accept, and dead code is judged only against current ones.
pub fn check_app(root: &Path) -> Result<Vec<Finding>, GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    check_model(root, &app)
}

pub(crate) fn check_model(root: &Path, app: &App) -> Result<Vec<Finding>, GenError> {
    let mut findings = check_loaded_app(root, app, &Screens::read(&app.modules))?;
    // Not in the build script: a build must not start failing as days pass.
    findings.extend(sofie_check::check_flag_ages(app, Date::today()));
    findings.extend(client_feature(root, app)?);
    sofie_check::sort_findings(&mut findings);
    Ok(findings)
}

/// Resolving the features reads the whole dependency graph, so only an app with a client area
/// pays for it.
fn client_feature(root: &Path, app: &App) -> Result<Vec<Finding>, GenError> {
    if !has_clients(app) {
        return Ok(Vec::new());
    }
    let features = sofie_model::framework_features(root).map_err(GenError::Model)?;
    Ok(sofie_check::check_client_feature(app, &features))
}

pub(crate) fn check_loaded_app(
    root: &Path,
    app: &App,
    screens: &Screens,
) -> Result<Vec<Finding>, GenError> {
    let languages = AppLanguages::read(app)?;
    let mut findings = sofie_check::check(app, screens);
    if let Some(languages) = &languages {
        findings.extend(languages.findings(app, screens)?);
    }
    if app.structure_ceilings {
        findings.extend(sofie_check::check_structure(root).map_err(GenError::Model)?);
    }
    if !findings.is_empty() {
        sofie_check::sort_findings(&mut findings);
        return Ok(findings);
    }
    let stale = [
        stale_files(
            sync_generated(app, screens, languages.as_ref(), GenMode::Check)?,
            Rule::StaleWiring,
            &format!("run `sofie gen {}`", root.display()),
        ),
        stale_docs(root, app)?,
    ]
    .concat();
    if !stale.is_empty() {
        return Ok(stale);
    }
    dead_code(root, app)
}

#[cfg(feature = "docs")]
fn stale_docs(root: &Path, app: &App) -> Result<Vec<Finding>, GenError> {
    if !crate::docs::keeps_docs(app) {
        return Ok(Vec::new());
    }
    Ok(stale_files(
        crate::docs::sync_docs(app, GenMode::Check)?,
        Rule::StaleDocs,
        &format!("run `sofie docs {}`", root.display()),
    ))
}

#[cfg(not(feature = "docs"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as with `docs`"
)]
fn stale_docs(_root: &Path, _app: &App) -> Result<Vec<Finding>, GenError> {
    Ok(Vec::new())
}

fn stale_files(outcomes: Vec<GenOutcome>, rule: Rule, fix: &str) -> Vec<Finding> {
    outcomes
        .into_iter()
        .filter_map(|outcome| match outcome {
            GenOutcome::Stale(file) => Some(Finding {
                rule,
                file,
                line: 1,
                message: "the generated file does not match the app".to_owned(),
                fix: fix.to_owned(),
            }),
            GenOutcome::Written(_) | GenOutcome::Removed(_) | GenOutcome::UpToDate(_) => None,
        })
        .collect()
}

fn dead_code(root: &Path, app: &App) -> Result<Vec<Finding>, GenError> {
    let derives = sofie_model::dependency_crate(root, DERIVES_PACKAGE).map_err(GenError::Model)?;
    let allowed = AllowList::read(root).map_err(GenError::Model)?;
    let checked: Vec<&Crate> = std::iter::once(&app.app)
        .chain(
            app.modules
                .iter()
                .flat_map(|module| std::iter::once(&module.krate).chain(&module.contract)),
        )
        .collect();
    let context: Vec<&Crate> = derives.iter().collect();
    let mut findings = check_dead_code(&checked, &context, &allowed, MAX_UNUSED_PUBLIC_RATIO)
        .map_err(GenError::Model)?;
    sofie_check::sort_findings(&mut findings);
    Ok(findings)
}
