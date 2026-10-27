use std::fmt::Write as _;

use crate::scaffold_tests::test_places;

/// The name `.mcp.json` registers the server under, which prefixes each tool's name in clients.
pub(super) const SERVER_NAME: &str = "sofie";

struct Step {
    name: &'static str,
    guidance: &'static str,
    tools: &'static [&'static str],
}

const STEPS: &[Step] = &[
    Step {
        name: "Read",
        guidance: "`modules` first, then `explain` on any name (route, screen, event, job, \
                   component).",
        tools: &[
            "modules",
            "explain",
            "routes",
            "schema",
            "screens",
            "components",
            "tasks",
            "flows",
            "docs_page",
        ],
    },
    Step {
        name: "Change",
        guidance: "a `new_*` tool, `edit_screen`, `scaffold_screen` or `feature`, each first \
                   with `dry_run`, then without; each runs `sofie gen` and `sofie check` after \
                   it. After a hand edit, `gen`.",
        tools: &[
            "new_route",
            "edit_screen",
            "new_aggregate",
            "new_migration",
            "new_event",
            "new_listener",
            "new_task",
            "new_menu_entry",
            "scaffold_screen",
            "feature",
            "gen",
        ],
    },
    Step {
        name: "Change, less often",
        guidance: "the other generators.",
        tools: &[
            "new_module",
            "new_area",
            "new_search",
            "new_plan",
            "new_mail",
            "new_account_screen",
            "new_impersonation",
            "new_tenancy",
        ],
    },
    Step {
        name: "Run",
        guidance: "`dev` starts the app, `request` tries a route on it, `logs` and `traces` \
                   show what it did; `task` and `db_status` run the app's binary once. A `request` \
                   `session` is signed in only for this `sofie mcp` process.",
        tools: &["request", "dev", "logs", "traces", "task", "db_status"],
    },
    Step {
        name: "Test",
        guidance: concat!(
            "`tests` (with `name`, one test's source) to follow the existing style, \
             `scaffold_tests` for missing ones, `test` to run them. ",
            test_places!()
        ),
        tools: &["tests", "scaffold_tests", "test"],
    },
    Step {
        name: "Finish",
        guidance: "`check` (`fix` applies its one-form fixes), `lint`, then `changes` to \
                   review every file changed.",
        tools: &["check", "fix", "lint", "changes", "build"],
    },
    Step {
        name: "Rarely",
        guidance: "`split`, `fuzz`, `seed`.",
        tools: &["split", "fuzz", "seed"],
    },
];

pub(super) fn instructions() -> String {
    let mut text = String::from(
        "Tools over one app built on the sofie framework; paths are relative to the app root. \
         Each description starts with its step. A client that defers tools loads a step's \
         tools at once, in one search for the names under it; adding a feature takes the names of \
         Read, Change, Test and Finish together.\n",
    );
    for step in STEPS {
        let _ = writeln!(text, "{}: {}", step.name, step.guidance);
        let names: Vec<String> = step
            .tools
            .iter()
            .map(|tool| format!("mcp__{SERVER_NAME}__{tool}"))
            .collect();
        let _ = writeln!(text, "  select:{}", names.join(","));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{STEPS, instructions};
    use crate::mcp::tools::definitions;

    #[test]
    fn each_tool_is_loaded_by_exactly_one_step_under_its_client_name() {
        let text = instructions();

        for tool in definitions() {
            let name = tool["name"].as_str().unwrap();
            let steps = STEPS
                .iter()
                .filter(|step| step.tools.contains(&name))
                .count();
            assert_eq!(steps, 1, "`{name}` is in {steps} steps");
            let loaded = format!("mcp__sofie__{name}");
            assert!(
                text.split([':', ',', '\n']).any(|part| part == loaded),
                "{text}"
            );
        }
        let tools = definitions();
        for name in STEPS.iter().flat_map(|step| step.tools) {
            assert!(
                tools.iter().any(|tool| tool["name"] == *name),
                "`{name}` is in a step but is no tool"
            );
        }
        assert!(
            text.contains("Test: `tests`") && text.contains("`app/tests/scaffolded/`"),
            "{text}"
        );
    }
}
