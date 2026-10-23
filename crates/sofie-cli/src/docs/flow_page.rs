use std::fmt::Write as _;

use sofie_model::App;

use super::markdown::{code, location, mermaid, mermaid_text};
use crate::flows::{Flow, Step, flows};

pub(super) fn flow_page(app: &App) -> String {
    let mut text = String::from(
        "# Request flows\n\nEach endpoint and schedule followed through the functions and \
         aggregate methods it calls, the events raised, their listeners and the jobs queued. \
         Branches are not drawn: a flow shows what can happen, not under which condition. \
         Edges the model cannot follow are listed below each diagram.\n",
    );
    for flow in flows(app) {
        text.push_str(&flow_section(app, &flow));
    }
    text
}

fn flow_section(app: &App, flow: &Flow) -> String {
    let mut text = format!("\n## {}\n\n{}\n", code(&flow.name), code(&flow.trigger));
    if let Some(doc) = entry_doc(app, &flow.entry) {
        let _ = write!(text, "\n{doc}\n");
    }
    text.push('\n');
    text.push_str(&mermaid("sequenceDiagram", &diagram_lines(flow)));
    if !flow.unresolved.is_empty() {
        text.push_str("\nUnresolved:\n\n");
        for unresolved in &flow.unresolved {
            let _ = writeln!(
                text,
                "- {}: {}",
                code(&location(&app.root, &unresolved.file, unresolved.line)),
                unresolved.reason
            );
        }
    }
    text
}

fn entry_doc(app: &App, entry: &Step) -> Option<String> {
    let Step::Function { module, path } = entry else {
        return None;
    };
    app.modules
        .iter()
        .find(|candidate| candidate.krate.crate_name == *module)?
        .code
        .function(path)?
        .doc
        .clone()
}

/// Numbered so labels may hold any text.
fn diagram_lines(flow: &Flow) -> Vec<String> {
    let steps = flow.edges.iter().flat_map(|edge| [&edge.from, &edge.to]);
    let participants = crate::sorted::first_seen(std::iter::once(&flow.entry).chain(steps));
    let index = |step: &Step| {
        participants
            .iter()
            .position(|known| *known == step)
            .unwrap_or_default()
    };
    let caller = if flow.trigger.starts_with("every ") {
        "participant Caller as Worker"
    } else {
        "actor Caller"
    };
    let mut lines = vec![caller.to_owned()];
    lines.extend(
        participants.iter().enumerate().map(|(number, step)| {
            format!("participant S{number} as {}", mermaid_text(&step.label()))
        }),
    );
    lines.push(format!("Caller->>S0: {}", mermaid_text(&flow.trigger)));
    for edge in &flow.edges {
        let arrow = if edge.kind.is_deferred() {
            "--)"
        } else {
            "->>"
        };
        let mut verb = edge.kind.verb().to_owned();
        if edge.repeated {
            verb.push_str(" (followed above)");
        }
        lines.push(format!(
            "S{}{arrow}S{}: {verb}",
            index(&edge.from),
            index(&edge.to)
        ));
    }
    lines
}
