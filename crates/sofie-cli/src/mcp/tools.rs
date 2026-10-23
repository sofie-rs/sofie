//! One table, so the tool list and the calls agree.

use std::path::Path;

use serde_json::{Map, Value, json};
use sofie_core::Timestamp;

use super::cargo::{self, CargoCommand};
use super::edit::ScreenEdit;
use super::generate::{self, Generator};
use super::request::{self, Call};
use super::session::{self, DevSession};
use super::{ToolOutput, changes, read, task, traffic};
use crate::feature_command::FeatureChange;
use crate::new_command::{NEW_COMMANDS, NewCommand};
use crate::scaffold_tests::test_places;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Flag,
    Number,
}

impl Kind {
    fn schema_type(self) -> &'static str {
        match self {
            Self::Text => "string",
            Self::Flag => "boolean",
            Self::Number => "integer",
        }
    }
}

struct Parameter {
    name: &'static str,
    kind: Kind,
    required: bool,
    description: &'static str,
}

/// Model tools read the app from its root; app tools act on the running app.
struct Tool<Run = fn(&Path, &Arguments) -> ToolOutput> {
    name: &'static str,
    description: &'static str,
    parameters: &'static [Parameter],
    run: Run,
}

type AppTool = Tool<fn(&mut DevSession, &Arguments) -> ToolOutput>;

pub(super) struct Arguments<'call> {
    values: &'call Map<String, Value>,
}

impl Arguments<'_> {
    fn text(&self, name: &str) -> &str {
        self.values.get(name).and_then(Value::as_str).unwrap_or("")
    }

    fn optional_text(&self, name: &str) -> Option<&str> {
        self.values.get(name).and_then(Value::as_str)
    }

    fn number(&self, name: &str) -> Option<u64> {
        self.values.get(name).and_then(Value::as_u64)
    }

    fn flag(&self, name: &str) -> bool {
        self.values
            .get(name)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

const fn text(name: &'static str, description: &'static str) -> Parameter {
    Parameter {
        name,
        kind: Kind::Text,
        required: true,
        description,
    }
}

const fn optional(name: &'static str, kind: Kind, description: &'static str) -> Parameter {
    Parameter {
        name,
        kind,
        required: false,
        description,
    }
}

const MODULE: Parameter = text("module", "The module's name, as in `modules/<module>/`.");

const SCREEN_KEY: Parameter = text(
    "key",
    "The screen as `<module>/<area>/<screen>`; the `screens` tool lists them.",
);

const DRY_RUN: Parameter = optional(
    "dry_run",
    Kind::Flag,
    "Run on a copy of the app and report the files it would create, change or \
                  remove and what `sofie check` would find; nothing is written.",
);

const GENERATOR_USE: &str = "Change: use to add this kind of code; pass `dry_run` first. ";

const TOOLS: &[Tool] = &[
    Tool {
        name: "explain",
        description: "Read: use first to learn about one named thing before changing it. Everything the app model knows about a route (`order.member.list_orders` \
                      or handler path; with the input it takes: each field, whether it is \
                      required and its JSON shape, and a body it takes; and the functions, \
                      events and jobs it reaches), area, module, event, job, read model, aggregate, screen (`order/member/orders`; its data, the paths its \
                      template reads, the route each `<Form>` and `<Action>` posts with that \
                      route's input, and its markup), screen component (`Table`) or flag \
                      (`order.split_payments`).",
        parameters: &[text("name", "What to explain.")],
        run: |root, arguments| read::explain(root, arguments.text("name")),
    },
    Tool {
        name: "modules",
        description: "Read: use first to see the app's modules and features. The app with the sofie features it enables, then each module: its tier, \
                      areas, events, aggregates, read models, jobs and route count.",
        parameters: &[],
        run: |root, _| read::modules(root),
    },
    Tool {
        name: "routes",
        description: "Read: use to find a route's path, name or handler. The route table: method, path, name, handler, permission and modifiers; \
                      `explain` on a route names the input it takes. Its paths are full, while \
                      generators take a path relative to the area's prefix.",
        parameters: &[],
        run: |root, _| read::routes(root),
    },
    Tool {
        name: "tasks",
        description: "Read: use before running or adding a task. Each `#[task]`: its name, the `--name value` arguments its input reads, \
                      its function and description.",
        parameters: &[],
        run: |root, _| read::tasks(root),
    },
    Tool {
        name: "components",
        description: "Read: use before writing or editing screen markup. Every component a screen template may use (`Section`, `Table`, `Form`, \
                      `Field`, …): what it is for, its props with their kinds, and its children; \
                      plus the template syntax around them, and which features this app has on.",
        parameters: &[],
        run: |root, _| {
            let enabled = sofie_model::framework_features(root).ok();
            ToolOutput::done(crate::component_list(enabled.as_deref()))
        },
    },
    Tool {
        name: "schema",
        description: "Read: use before changing data, fields or migrations. Each module's aggregates with their fields and columns, its read models \
                      and its migrations.",
        parameters: &[],
        run: |root, _| read::schema(root),
    },
    Tool {
        name: "screens",
        description: "Read: use to find a screen's key before editing it. Each screen template by key, with the routes that show it.",
        parameters: &[],
        run: |root, _| read::screens(root),
    },
    Tool {
        name: "flows",
        description: "Read: use to see what a request sets off. Every request flow: endpoint, action, method, event, listener, job.",
        parameters: &[],
        run: |root, _| read::docs_page(root, "flows"),
    },
    Tool {
        name: "tests",
        description: "Test: use before writing a test, to follow the existing ones. Each test file of the app and its modules with its test functions, as \
                      `#[attribute] path::name`: where tests live and the style to follow.",
        parameters: &[optional(
            "name",
            Kind::Text,
            "One test's name or path, as `cancels` or `tests::adds`: answers its file, lines \
             and source instead of the list.",
        )],
        run: |root, arguments| read::tests(root, arguments.optional_text("name")),
    },
    Tool {
        name: "check",
        description: "Finish: use after every change; it must answer ok. Runs `sofie check`: every structure rule, then stale generated files, then \
                      dead code.",
        parameters: &[],
        run: |root, _| read::check(root),
    },
    Tool {
        name: "docs_page",
        description: "Read: use for a generated overview page. A page `sofie docs` writes, by name: `README`, `architecture`, `flows` or \
                      `modules/<module>`.",
        parameters: &[text(
            "page",
            "The page's path under `docs/gen/` without `.md`.",
        )],
        run: |root, arguments| read::docs_page(root, arguments.text("page")),
    },
    Tool {
        name: "changes",
        description: "Finish: use to review your change set. The files added, modified, renamed \
                      or deleted under the app root since git's last commit, read with `git \
                      status`; with `path`, that file's diff, or a new file's text.",
        parameters: &[optional(
            "path",
            Kind::Text,
            "A changed file relative to the app root, as `app/src/lib.rs`.",
        )],
        run: |root, arguments| changes::changes(root, arguments.optional_text("path")),
    },
    Tool {
        name: "new_module",
        description: "Change: use to add a module; rarely needed. `sofie new module`: a module with its manifest, contract, config and a `web` \
                      area, which the app then depends on; then `sofie gen` and `sofie check`.",
        parameters: &[
            text(
                "name",
                "The module's name: lowercase letters, digits and `_`.",
            ),
            DRY_RUN,
        ],
        run: |root, arguments| {
            let generator = Generator::NewModule(arguments.text("name").to_owned());
            generate::run(root, &generator, arguments.flag(DRY_RUN.name))
        },
    },
    Tool {
        name: "scaffold_tests",
        description: concat!(
            "Test: use after adding code, to write its missing tests. `sofie scaffold tests`: \
             tests for what the modules declare that no test covers yet, in a new file named by \
             the moment it is written (`<module>_<YYYYMMDDHHMMSS>.rs`); then `sofie gen` and \
             `sofie check`. ",
            test_places!()
        ),
        parameters: &[
            DRY_RUN,
            optional(
                "all",
                Kind::Flag,
                "List every gap no test can be scaffolded for; without it, gaps of subjects \
                 earlier files already test are counted on one line.",
            ),
        ],
        run: |root, arguments| {
            let generator = Generator::ScaffoldTests {
                now: Timestamp::now(),
                all: arguments.flag("all"),
            };
            generate::run(root, &generator, arguments.flag(DRY_RUN.name))
        },
    },
    Tool {
        name: "gen",
        description: "Change: use after editing code by hand. `sofie gen`: rewrites the wiring, contracts' generated code, `.env.example` \
                      files and `AGENTS.md`; then `sofie check`.",
        parameters: &[DRY_RUN],
        run: |root, arguments| generate::run(root, &Generator::Gen, arguments.flag(DRY_RUN.name)),
    },
    Tool {
        name: "fix",
        description: "Finish: use when `check` reports findings with one fix. `sofie fix`: applies the fixes with exactly one form (a missing `mod` line, \
                      app dependency or re-export, stale generated files); then `sofie gen` and \
                      `sofie check`.",
        parameters: &[DRY_RUN],
        run: |root, arguments| generate::run(root, &Generator::Fix, arguments.flag(DRY_RUN.name)),
    },
    Tool {
        name: "feature",
        description: "Change: use when `components`, a build error or a doc names a sofie \
                      feature this app has off. `sofie feature add|remove`: edits only the \
                      features array of the app's `sofie` dependency in `app/Cargo.toml`, \
                      checked against the features `sofie` declares; then `sofie gen` and \
                      `sofie check`.",
        parameters: &[
            text("action", "`add` or `remove`."),
            text(
                "name",
                "The feature, as `i18n`; an unknown one answers the list.",
            ),
            DRY_RUN,
        ],
        run: |root, arguments| {
            let change = match arguments.text("action") {
                "add" => FeatureChange::Add,
                "remove" => FeatureChange::Remove,
                other => {
                    return ToolOutput::failed(format!(
                        "`feature` takes the action `add` or `remove`, not `{other}`"
                    ));
                }
            };
            let generator = Generator::Feature(arguments.text("name").to_owned(), change);
            generate::run(root, &generator, arguments.flag(DRY_RUN.name))
        },
    },
];

/// After the generators of `NEW_COMMANDS`.
const LATER_TOOLS: &[Tool] = &[
    Tool {
        name: "scaffold_screen",
        description: "Change: use to customize a built-in resource screen. `sofie scaffold screen`: a resource's built-in screen written to the module's \
                      area, which then overrides it; builds the app to read it. Then `sofie gen` \
                      and `sofie check`.",
        parameters: &[SCREEN_KEY, DRY_RUN],
        run: |root, arguments| {
            let generator = Generator::ScaffoldScreen(arguments.text(SCREEN_KEY.name).to_owned());
            generate::run(root, &generator, arguments.flag(DRY_RUN.name))
        },
    },
    Tool {
        name: "edit_screen",
        description: "Change: use to change a screen template's markup. Changes a module's screen template: replaces the one occurrence of the \
                      exact text `find` (copied from `explain <key>`'s markup) with `replace`, and \
                      refuses when `find` is missing or occurs more than once. Then `sofie gen` and `sofie check`.",
        parameters: &[
            SCREEN_KEY,
            text(
                "find",
                "The exact text to replace, whitespace included; it must occur once.",
            ),
            text("replace", "The text to put in its place; empty removes it."),
            DRY_RUN,
        ],
        run: |root, arguments| {
            let generator = Generator::EditScreen(ScreenEdit {
                key: arguments.text(SCREEN_KEY.name).to_owned(),
                find: arguments.text("find").to_owned(),
                replace: arguments.text("replace").to_owned(),
            });
            generate::run(root, &generator, arguments.flag(DRY_RUN.name))
        },
    },
    Tool {
        name: "split",
        description: "Rarely: use to move a module into an app of its own. `sofie split`: the plan for moving a module to an app of its own under \
                      `split/<module>/`, its contracts, lookups and events served over HTTP, \
                      and the reverse-proxy table; with `write`, carries it out and runs \
                      `sofie gen` in both apps.",
        parameters: &[
            MODULE,
            optional(
                "write",
                Kind::Flag,
                "Move the module and write both apps; without it only the plan.",
            ),
        ],
        run: |root, arguments| split(root, arguments.text("module"), arguments.flag("write")),
    },
    Tool {
        name: "build",
        description: "Finish: use to see compile errors without running tests. `cargo build --offline` of the app and its modules; answers the errors \
                      and warnings, bounded.",
        parameters: &[],
        run: |root, _| cargo::run(root, CargoCommand::Build, None),
    },
    Tool {
        name: "test",
        description: "Test: use after a change to run the app's tests. `cargo test --offline` of the app and its modules; answers the \
                      counts of passed, failed and ignored tests, the diagnostics, every failure \
                      in full, and the passed and ignored tests by file: every name in files git \
                      sees changed, counts elsewhere; with `filter`, every name it ran and the \
                      tests in changed files it left out, and fails when it matched no test.",
        parameters: &[optional(
            "filter",
            Kind::Text,
            "Runs only the tests whose path contains it. A path is within its test \
                          binary: a test in `app/tests/orders.rs` is plain `test_name`, one in a \
                          module the binary declares `module::test_name`; a file name is no \
                          prefix.",
        )],
        run: |root, arguments| {
            cargo::run(root, CargoCommand::Test, arguments.optional_text("filter"))
        },
    },
    Tool {
        name: "lint",
        description: "Finish: use last, to format and lint the change. `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` of the \
                      app and its modules alone; answers the files needing formatting and \
                      clippy's findings, bounded.",
        parameters: &[optional(
            "write",
            Kind::Flag,
            "Format the files with `cargo fmt` instead of only listing them.",
        )],
        run: |root, arguments| cargo::lint(root, arguments.flag("write")),
    },
    Tool {
        name: "db_status",
        description: "Run: use when migrations seem out of step. Builds the development binary and runs `<app> db status` with the app's \
                      `.env`: per module each migration applied, pending, edited after being \
                      applied or missing as a file; it fails when one is edited or missing.",
        parameters: &[],
        run: |root, _| task::db_status(root),
    },
    Tool {
        name: "task",
        description: "Run: use to run a `#[task]` once. Builds the development binary and runs `<app> task NAME` once with the \
                      app's `.env`, against its database: answers the task's output and span \
                      tree, and fails with its error.",
        parameters: &[
            text(
                "name",
                "The task as `<module>.<function>`; the `tasks` tool lists them.",
            ),
            optional(
                "arguments",
                Kind::Text,
                "A JSON object of the task's arguments, as \
                              `{\"since\": \"2026-01-01\"}`.",
            ),
        ],
        run: |root, arguments| {
            task::run(
                root,
                arguments.text("name"),
                arguments.optional_text("arguments"),
            )
        },
    },
    Tool {
        name: "fuzz",
        description: "Rarely: use to hunt crashes across every route. `sofie fuzz`: runs the app on a free local port with the shield off, signs \
                      members up and in, and sends every route generated inputs (valid, \
                      boundary, malformed, oversized, wrong types, missing and unknown fields, \
                      foreign and random ids) as page, JSON and fragment; answers each 500, \
                      dropped answer or crash with its input, span tree and log lines. Only on \
                      a database on this machine.",
        parameters: &[
            optional(
                "seed",
                Kind::Number,
                "Sends the same requests as an earlier run with this seed.",
            ),
            optional(
                "requests",
                Kind::Number,
                "How many requests; 300 when none.",
            ),
        ],
        run: |root, arguments| {
            traffic::fuzz_app(root, arguments.number("seed"), arguments.number("requests"))
        },
    },
    Tool {
        name: "seed",
        description: "Rarely: use to fill the development database with records. `sofie seed`: posts each module's `seeds/<area>.<handler>.json` and `volume` \
                      generated valid inputs to every POST route taking an input, as the member \
                      `sofie bench` signs in with; answers the records made and refused per route. \
                      Only on a database on this machine.",
        parameters: &[optional(
            "volume",
            Kind::Number,
            "Generated records per route; none when absent.",
        )],
        run: |root, arguments| traffic::seed_app(root, arguments.number("volume")),
    },
];

const APP_TOOLS: &[AppTool] = &[
    Tool {
        name: "dev",
        description: "Run: use to start, stop or check the app, or run a scheduled job now. Runs the app as `sofie dev` does (built with `sofie/development`, on a free \
                      local port): `start` builds and starts it, `stop` stops it, `status` \
                      answers its pid, address, whether it runs and its last exit, and `run` \
                      queues a schedule's job now, which the running worker takes next, \
                      whatever its interval.",
        parameters: &[
            text("action", "`start`, `stop`, `status` or `run`."),
            optional(
                "schedule",
                Kind::Text,
                "With `run`: the schedule's name, as its module registers it \
                              (`order.jobs.unpaid_orders`); an unknown one answers the names.",
            ),
        ],
        run: |session, arguments| {
            session::dev(
                session,
                arguments.text("action"),
                arguments.optional_text("schedule"),
            )
        },
    },
    Tool {
        name: "logs",
        description: "Run: use to read the running app's output. The running app's stdout and stderr lines after a cursor, numbered; the \
                      last 2000 are kept. Answers the next cursor.",
        parameters: &[
            optional(
                "after",
                Kind::Number,
                "The cursor a previous call answered; 0 or none for the oldest.",
            ),
            optional("filter", Kind::Text, "Only lines containing this text."),
        ],
        run: |session, arguments| {
            session::logs(
                session,
                arguments.number("after").unwrap_or(0),
                arguments.optional_text("filter"),
            )
        },
    },
    Tool {
        name: "traces",
        description: "Run: use to see what a request or job did, span by span. Span trees the running app logged: without `trace` the recent trees' \
                      first lines with their trace ids; with it every tree of that trace (a \
                      request's, then the jobs and listeners that joined it).",
        parameters: &[optional(
            "trace",
            Kind::Text,
            "A trace id: 32 lowercase hexadecimal digits.",
        )],
        run: |session, arguments| session::traces(session, arguments.optional_text("trace")),
    },
    Tool {
        name: "request",
        description: "Run: use to try a route on the running app. Sends a request to the running app, starting it or rebuilding it first \
                      when its code changed, with an `Origin` that passes the CSRF check; \
                      answers the status, `location`, `content-type` and `x-request-id`, the \
                      names of cookies set, the request's span tree and the body (cut at 16 KiB). \
                      `explain` on the route names the body it takes. sofie-identity's sign-up \
                      answers a taken address exactly like a new one and creates nothing, so \
                      sign up with a fresh address.",
        parameters: &[
            text("path", "The path with its query, such as `/orders?page=2`."),
            optional("method", Kind::Text, "`GET` when none."),
            optional(
                "body",
                Kind::Text,
                "Sent as JSON when it starts with `{`, else as a form \
                              (`email=a%40b.test&name=Ada`).",
            ),
            optional(
                "answer",
                Kind::Text,
                "`page` (the default), `json` or `fragment` (as htmx asks).",
            ),
            optional(
                "target",
                Kind::Text,
                "The fragment's id, as a `Form`'s or `Action`'s `target` names it \
                              (`orders`); sent as `hx-target`, so the app answers that fragment \
                              alone. Implies `answer` `fragment`.",
            ),
            optional(
                "session",
                Kind::Text,
                "A cookie jar by name: sign in through the app's own routes with \
                              one, then act as that principal by naming it again.",
            ),
        ],
        run: |session, arguments| {
            let call = Call {
                method: arguments.optional_text("method").unwrap_or("GET"),
                path: arguments.text("path"),
                body: arguments.optional_text("body"),
                answer: arguments.optional_text("answer"),
                target: arguments.optional_text("target"),
                session: arguments.optional_text("session"),
            };
            request::request(session, &call)
        },
    },
];

pub(super) fn definitions() -> Vec<Value> {
    let generators = NEW_COMMANDS.iter().map(|command| {
        definition(
            command.tool,
            &format!("{GENERATOR_USE}{}", command.description),
            &generator_parameters(command),
        )
    });
    let model = |tool: &Tool| definition(tool.name, tool.description, tool.parameters);
    let app = APP_TOOLS
        .iter()
        .map(|tool| definition(tool.name, tool.description, tool.parameters));
    TOOLS
        .iter()
        .map(model)
        .chain(generators)
        .chain(LATER_TOOLS.iter().map(model))
        .chain(app)
        .collect()
}

fn generator_parameters(command: &NewCommand) -> Vec<Parameter> {
    let arguments = command.arguments.iter().map(|argument| Parameter {
        name: argument.name,
        kind: Kind::Text,
        required: !argument.optional,
        description: argument.description,
    });
    arguments.chain([DRY_RUN]).collect()
}

fn definition(name: &str, description: &str, parameters: &[Parameter]) -> Value {
    let properties: Map<String, Value> = parameters
        .iter()
        .map(|parameter| {
            (
                parameter.name.to_owned(),
                json!({ "type": parameter.kind.schema_type(), "description": parameter.description }),
            )
        })
        .collect();
    let required: Vec<&str> = parameters
        .iter()
        .filter(|parameter| parameter.required)
        .map(|parameter| parameter.name)
        .collect();
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

pub(super) fn call(
    root: &Path,
    session: &mut DevSession,
    name: &str,
    values: &Map<String, Value>,
) -> Result<ToolOutput, String> {
    let arguments = Arguments { values };
    let output = if let Some(tool) = TOOLS
        .iter()
        .chain(LATER_TOOLS)
        .find(|tool| tool.name == name)
    {
        check_arguments(name, tool.parameters, values)?;
        (tool.run)(root, &arguments)
    } else if let Some(command) = NEW_COMMANDS.iter().find(|command| command.tool == name) {
        check_arguments(name, &generator_parameters(command), values)?;
        match command.build(|argument| arguments.optional_text(argument).map(str::to_owned)) {
            Ok(code) => generate::run(root, &Generator::New(code), arguments.flag(DRY_RUN.name)),
            Err(error) => ToolOutput::failed(error),
        }
    } else if let Some(tool) = APP_TOOLS.iter().find(|tool| tool.name == name) {
        check_arguments(name, tool.parameters, values)?;
        (tool.run)(session, &arguments)
    } else {
        return Err(format!(
            "`sofie mcp` has no tool `{name}`; `tools/list` names them"
        ));
    };
    Ok(output.finished(root))
}

fn check_arguments(
    tool: &str,
    parameters: &[Parameter],
    values: &Map<String, Value>,
) -> Result<(), String> {
    if let Some(unknown) = values
        .keys()
        .find(|key| parameters.iter().all(|parameter| parameter.name != *key))
    {
        return Err(format!("tool `{tool}` takes no argument `{unknown}`"));
    }
    for parameter in parameters {
        let fits = match (values.get(parameter.name), parameter.kind) {
            (None, _) => !parameter.required,
            (Some(Value::String(_)), Kind::Text) | (Some(Value::Bool(_)), Kind::Flag) => true,
            (Some(Value::Number(number)), Kind::Number) => number.is_u64(),
            (Some(_), _) => false,
        };
        if !fits {
            return Err(format!(
                "tool `{tool}` needs `{}` as a {}",
                parameter.name,
                parameter.kind.schema_type()
            ));
        }
    }
    Ok(())
}

fn split(root: &Path, module: &str, write: bool) -> ToolOutput {
    if !write {
        return match crate::split_command::split_plan(root, module) {
            Ok(plan) => ToolOutput::done(plan.to_string()),
            Err(error) => ToolOutput::from_error(root, &error),
        };
    }
    match crate::split_command::split_module(root, module) {
        Ok(split) => ToolOutput::done(format!(
            "{}\nwritten; `sofie gen` ran in both apps\n",
            split.plan
        )),
        Err(error) => ToolOutput::from_error(root, &error),
    }
}
