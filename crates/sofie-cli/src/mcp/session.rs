use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::ToolOutput;
use crate::cargo::Diagnostics;
use crate::gen_command::GenError;
use crate::http_client::CookieJars;
use crate::local_app::{LocalApp, refuse_remote_database};
use crate::log_buffer::{self, LogBuffer, TREES_PER_KIND, Tree};
use crate::schedule_command::run_schedule_with;

const LOG_PAGE: usize = 200;
const RECENT_TREES: usize = 20;

/// What `sofie mcp` keeps between calls: the development app it runs, its
/// output and the request tool's cookie jars.
pub(super) struct DevSession {
    root: PathBuf,
    pub(super) log: Arc<LogBuffer>,
    app: Option<LocalApp>,
    pub(super) jars: CookieJars,
}

impl DevSession {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            log: Arc::default(),
            app: None,
            jars: CookieJars::default(),
        }
    }

    /// Starts the app, or rebuilds it when its code changed or its last
    /// build failed; answers where it listens once `/live` does.
    pub(super) fn current(&mut self) -> Result<SocketAddr, GenError> {
        refuse_remote_database(&self.root, "mcp")?;
        let local = match &mut self.app {
            Some(local) => local,
            None => self
                .app
                .insert(LocalApp::new(&self.root, Arc::clone(&self.log))?),
        };
        let app = &mut local.app;
        app.exited()?;
        let restarted = if app.pid().is_none() || app.build_failed() {
            app.rebuild().map(Some)
        } else {
            app.refresh()
        };
        if restarted?.is_some() {
            local.wait_live()?;
        }
        Ok(local.address)
    }

    fn stop(&mut self) -> ToolOutput {
        match &mut self.app {
            Some(local) if local.app.pid().is_some() => {
                local.app.stop();
                ToolOutput::done("stopped the app".to_owned())
            }
            _ => ToolOutput::done("the app is not running".to_owned()),
        }
    }

    fn status(&mut self) -> String {
        let Some(local) = &mut self.app else {
            return "not started; `dev` with `start`, or any `request`, starts it".to_owned();
        };
        let origin = local.origin();
        let app = &mut local.app;
        if let Err(error) = app.exited() {
            return error.to_string();
        }
        let mut text = match app.pid() {
            Some(pid) => format!("running: pid {pid}, address {origin}"),
            None => "not running".to_owned(),
        };
        if let Some(status) = app.last_exit() {
            let _ = write!(text, "\nlast exit: {status}");
        }
        if app.build_failed() {
            text.push_str("\nthe last build failed; the next start or request rebuilds");
        }
        text
    }
}

pub(super) fn dev(session: &mut DevSession, action: &str, schedule: Option<&str>) -> ToolOutput {
    match (action, schedule) {
        ("start", _) => match session.current() {
            Ok(_) => ToolOutput::done(session.status()),
            Err(error) => ToolOutput::failed(error.to_string()),
        },
        ("stop", _) => session.stop(),
        ("status", _) => ToolOutput::done(session.status()),
        ("run", Some(name)) => run_schedule(session, name),
        ("run", None) => ToolOutput::failed(
            "`run` takes `schedule`, the name its job is registered under, as \
             `order.jobs.unpaid_orders`"
                .to_owned(),
        ),
        (other, _) => ToolOutput::failed(format!(
            "`dev` takes the action `start`, `stop`, `status` or `run`, not `{other}`"
        )),
    }
}

/// The app runs first, so its worker takes the queued job.
fn run_schedule(session: &mut DevSession, name: &str) -> ToolOutput {
    if let Err(error) = session.current() {
        return ToolOutput::failed(format!("the app is not running: {error}"));
    }
    match run_schedule_with(&session.root, name, Diagnostics::Captured) {
        Ok(line) => ToolOutput::done(format!("{line}\n`traces` shows its run")),
        Err(error) => ToolOutput::from_error(&session.root, &error),
    }
}

pub(super) fn logs(session: &DevSession, after: u64, filter: Option<&str>) -> ToolOutput {
    let page = session.log.after(after, filter, LOG_PAGE);
    let mut text = String::new();
    if page.dropped {
        let _ = writeln!(
            text,
            "(older lines were dropped; the buffer keeps the last {}; `traces` keeps span trees apart)",
            log_buffer::CAPACITY
        );
    }
    for line in &page.lines {
        text.push_str(line);
        text.push('\n');
    }
    let _ = write!(text, "next cursor: {}", page.next);
    ToolOutput::done(text)
}

pub(super) fn traces(session: &DevSession, trace_id: Option<&str>) -> ToolOutput {
    let Some(trace_id) = trace_id else {
        let recent = session.log.recent(RECENT_TREES);
        let text = format!(
            "requests:\n{}\n\njobs and listeners:\n{}",
            tree_list(&recent.requests),
            tree_list(&recent.background)
        );
        return ToolOutput::done(text);
    };
    let found = session.log.trees(trace_id);
    if found.trees.is_empty() && found.dropped {
        return ToolOutput::failed(format!(
            "the span trees of trace {trace_id} were dropped: newer ones filled the {TREES_PER_KIND} kept of its kind"
        ));
    }
    if found.trees.is_empty() {
        return ToolOutput::failed(format!("no span tree of trace {trace_id} in the log yet"));
    }
    let text = found
        .trees
        .iter()
        .map(|tree| tree.lines.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n");
    ToolOutput::done(text)
}

fn tree_list(trees: &[Tree]) -> String {
    if trees.is_empty() {
        return "(none)".to_owned();
    }
    trees
        .iter()
        .map(|tree| format!("{} {}", tree.trace_id, tree.lines[0]))
        .collect::<Vec<_>>()
        .join("\n")
}
