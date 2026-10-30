#![cfg(feature = "mcp")]

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sofie_model::testing::{TemporaryDirectory, manifest};

fn shop() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shop")
}

fn exchange(root: &Path, messages: &[Value]) -> Vec<Value> {
    let lines: Vec<String> = messages.iter().map(Value::to_string).collect();
    answers(root, &lines.join("\n"))
}

fn answers(root: &Path, input: &str) -> Vec<Value> {
    let mut output = Vec::new();
    sofie_cli::serve_mcp(root, Cursor::new(input), &mut output).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn call(root: &Path, name: &str, arguments: &Value) -> (String, bool) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let answers = exchange(root, &[request]);
    let result = &answers[0]["result"];
    assert!(result.is_object(), "{}", answers[0]);
    (
        result["content"][0]["text"].as_str().unwrap().to_owned(),
        result["isError"].as_bool().unwrap(),
    )
}

#[test]
fn initialize_negotiates_the_version_and_notifications_get_no_answer() {
    let answers = exchange(
        &shop(),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                   "params": {"protocolVersion": "2025-03-26", "capabilities": {}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": "two", "method": "initialize",
                   "params": {"protocolVersion": "1999-01-01"}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}),
        ],
    );

    assert_eq!(answers.len(), 3, "{answers:?}");
    assert_eq!(answers[0]["id"], 1);
    assert_eq!(answers[0]["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(answers[0]["result"]["serverInfo"]["name"], "sofie");
    assert!(answers[0]["result"]["capabilities"]["tools"].is_object());
    assert_eq!(answers[1]["id"], "two");
    assert_eq!(answers[1]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answers[2], json!({"jsonrpc": "2.0", "id": 3, "result": {}}));
}

#[test]
fn tools_list_describes_every_tool_with_its_input_schema() {
    let answers = exchange(
        &shop(),
        &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})],
    );

    let tools = answers[0]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "explain",
            "modules",
            "routes",
            "tasks",
            "components",
            "schema",
            "screens",
            "flows",
            "tests",
            "check",
            "docs_page",
            "changes",
            "new_module",
            "scaffold_tests",
            "gen",
            "fix",
            "feature",
            "new_migration",
            "new_aggregate",
            "new_event",
            "new_listener",
            "new_task",
            "new_search",
            "new_plan",
            "new_mail",
            "new_area",
            "new_account_screen",
            "new_impersonation",
            "new_tenancy",
            "new_route",
            "new_menu_entry",
            "scaffold_screen",
            "edit_screen",
            "split",
            "build",
            "test",
            "lint",
            "db_status",
            "task",
            "fuzz",
            "seed",
            "dev",
            "logs",
            "traces",
            "request"
        ]
    );
    let new_module =
        &tools[names.iter().position(|name| *name == "new_module").unwrap()]["inputSchema"];
    assert_eq!(new_module["required"], json!(["name"]));
    assert_eq!(new_module["properties"]["dry_run"]["type"], "boolean");
}

#[test]
fn the_instructions_map_each_step_to_tools_and_each_description_starts_with_its_step() {
    let answers = exchange(
        &shop(),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        ],
    );

    let instructions = answers[0]["result"]["instructions"].as_str().unwrap();
    let tools = answers[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(instructions.lines().count() <= 25, "{instructions}");
    let mcp_json: Value =
        serde_json::from_str(&fs::read_to_string(shop().join(".mcp.json")).unwrap()).unwrap();
    let servers = mcp_json["mcpServers"].as_object().unwrap();
    let server = servers.keys().next().unwrap();
    assert!(
        instructions.contains(&format!(
            "select:mcp__{server}__modules,mcp__{server}__explain,"
        )),
        "{instructions}"
    );
    for step in ["Read:", "Change:", "Run:", "Test:", "Finish:"] {
        assert!(instructions.contains(step), "{step} in {instructions}");
    }
    let mentioned = instructions.split('`').skip(1).step_by(2).filter(|word| {
        word.chars()
            .all(|letter| letter.is_ascii_lowercase() || letter == '_')
    });
    for name in mentioned.filter(|name| !["dry_run", "name"].contains(name)) {
        assert!(
            names.contains(&name),
            "`{name}` is named in the instructions but is no tool"
        );
    }
    for tool in tools {
        let description = tool["description"].as_str().unwrap();
        let step = description.split_once(": use ").map(|(step, _)| step);
        assert!(
            step.is_some_and(
                |step| ["Read", "Change", "Run", "Test", "Finish", "Rarely"].contains(&step)
            ),
            "{description}"
        );
    }
}

#[test]
fn malformed_and_unknown_requests_are_json_rpc_errors() {
    let input = "not json\n\
                 {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"resources/list\"}\n\
                 {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"deploy\"}}\n\
                 {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"explain\",\"arguments\":{}}}\n\
                 {\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"gen\",\"arguments\":{\"dry_run\":\"yes\"}}}\n";

    let answers = answers(&shop(), input);

    let codes: Vec<(&Value, i64)> = answers
        .iter()
        .map(|answer| (&answer["id"], answer["error"]["code"].as_i64().unwrap()))
        .collect();
    assert_eq!(
        codes,
        [
            (&Value::Null, -32700),
            (&json!(1), -32601),
            (&json!(2), -32602),
            (&json!(3), -32602),
            (&json!(4), -32602),
        ]
    );
    assert_eq!(
        answers[3]["error"]["message"],
        "tool `explain` needs `name` as a string"
    );
}

/// A client waits for an answer to every request, and one bad line must not end the server.
#[test]
fn a_batch_a_non_object_and_a_line_that_is_not_utf8_are_answered_and_the_server_goes_on() {
    let mut input: Vec<u8> = b"[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}]\n42\n".to_vec();
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"p\xffing\"}\n");
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n");
    let mut output = Vec::new();

    sofie_cli::serve_mcp(&shop(), Cursor::new(input), &mut output).unwrap();

    let answers: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let codes: Vec<(&Value, Option<i64>)> = answers
        .iter()
        .map(|answer| (&answer["id"], answer["error"]["code"].as_i64()))
        .collect();
    assert_eq!(
        codes,
        [
            (&Value::Null, Some(-32600)),
            (&Value::Null, Some(-32600)),
            (&Value::Null, Some(-32700)),
            (&json!(3), None),
        ]
    );
}

#[test]
fn read_tools_answer_from_the_shop_s_model_with_relative_paths() {
    let root = shop();

    let (aggregate, _) = call(&root, "explain", &json!({"name": "Order"}));
    let (unknown, unknown_failed) = call(&root, "explain", &json!({"name": "Nothing"}));
    let (modules, _) = call(&root, "modules", &json!({}));
    let (routes, _) = call(&root, "routes", &json!({}));
    let (schema, _) = call(&root, "schema", &json!({}));
    let (screens, _) = call(&root, "screens", &json!({}));
    let (flows, _) = call(&root, "flows", &json!({}));
    let (page, _) = call(&root, "docs_page", &json!({"page": "modules/order"}));
    let (missing, missing_failed) = call(&root, "docs_page", &json!({"page": "nope"}));
    let (check, check_failed) = call(&root, "check", &json!({}));

    assert!(
        aggregate.starts_with(
            "aggregate Order\n  module: order\n  declared: modules/order/src/order/mod.rs:"
        ),
        "{aggregate}"
    );
    assert!(unknown_failed && unknown.contains("`Nothing`"), "{unknown}");
    assert!(
        modules.starts_with("app shop\n  path: app\n  module: order\n  sofie features: "),
        "{modules}"
    );
    assert!(
        modules.contains("\n\nmodule order\n  path: modules/order\n  tier: Core"),
        "{modules}"
    );
    assert!(routes.contains("order.customer.list_orders"), "{routes}");
    assert!(schema.contains("read model OrderRow"), "{schema}");
    assert!(
        schema.contains("migrations of order\n  modules/order/migrations/0000_module_role.sql\n  modules/order/migrations/0001_create_order.sql"),
        "{schema}"
    );
    assert!(
        screens.starts_with("screen order/customer/orders\n  template: modules/order/src/areas/customer/screens/orders.html"),
        "{screens}"
    );
    assert!(flows.contains("order.customer.place_order"), "{flows}");
    assert!(page.starts_with("# Module `order`"), "{page}");
    assert!(
        missing_failed
            && missing.contains("README, architecture, flows, security, telemetry, modules/order"),
        "{missing}"
    );
    assert_eq!((check.as_str(), check_failed), ("ok\n", false));
}

#[test]
fn explaining_a_screen_shows_its_loader_s_data_and_the_template_s_bindings() {
    let (screen, failed) = call(
        &shop(),
        "explain",
        &json!({"name": "order/customer/orders"}),
    );

    assert!(!failed, "{screen}");
    for expected in [
        "  data from `queries::list_orders` (order.customer.list_orders): Vec<OrderRow>\n",
        "    status: OrderStatus\n",
        "  binds: data, ",
        "data[].status_to.CANCELLED",
    ] {
        assert!(
            screen.contains(expected),
            "missing {expected:?} in {screen}"
        );
    }
}

#[test]
fn identity_routes_show_their_real_paths_and_explain_their_input() {
    let root = shop();

    let (routes, _) = call(&root, "routes", &json!({}));
    let (sign_up, failed) = call(
        &root,
        "explain",
        &json!({"name": "sofie_identity.customer.sign_up"}),
    );

    assert!(!routes.contains("{sign_in}"), "{routes}");
    assert!(!routes.contains("{prefix}"), "{routes}");
    assert!(routes.contains("/account/sign-out"), "{routes}");
    assert!(
        routes.contains("generators such as `sofie new route` take a path relative"),
        "{routes}"
    );
    assert!(!failed, "{sign_up}");
    for expected in [
        "route sofie_identity.customer.sign_up\n",
        "  POST /sign-up\n",
        "  input: Credentials (",
        "    email: ",
        "    password: ",
    ] {
        assert!(
            sign_up.contains(expected),
            "missing {expected:?} in {sign_up}"
        );
    }
}

#[test]
fn generate_tools_on_the_shop_dry_run_on_a_copy_and_leave_it_untouched() {
    let root = shop();

    let (module, module_failed) = call(
        &root,
        "new_module",
        &json!({"name": "billing", "dry_run": true}),
    );
    let (generated, generated_failed) = call(&root, "gen", &json!({"dry_run": true}));
    let (fixed, fixed_failed) = call(&root, "fix", &json!({"dry_run": true}));
    let (scaffolded, _) = call(&root, "scaffold_tests", &json!({"dry_run": true}));

    assert!(!root.join("modules/billing").exists());
    assert!(!module_failed, "{module}");
    for expected in [
        "dry run, nothing was written; it would:\n",
        "  change app/Cargo.toml\n",
        "  change app/src/wiring.rs\n",
        "  create modules/billing/src/module.rs\n",
        "  create docs/gen/modules/billing.md\n",
        "what the generators reported on a scratch copy, not this app:\ncreated module `billing`",
        "what it would write:\n",
        "create modules/billing/src/module.rs\n@@ -0,0 +1,",
    ] {
        assert!(
            module.contains(expected),
            "missing {expected:?} in {module}"
        );
    }
    assert!(module.contains("sofie check:\nok\n"), "{module}");
    let unchanged = "dry run, nothing was written; it would:\n  nothing\n\n";
    assert!(
        !generated_failed && generated.starts_with(unchanged),
        "{generated}"
    );
    assert!(!fixed_failed && fixed.starts_with(unchanged), "{fixed}");
    assert!(
        scaffolded.starts_with("dry run, nothing was written"),
        "{scaffolded}"
    );
    assert!(scaffolded.contains("sofie check:\nok\n"), "{scaffolded}");
}

#[test]
fn a_generate_tool_writes_then_runs_gen_and_check() {
    let directory = TemporaryDirectory::new("sofie-mcp-new");
    sofie_cli::new_app(directory.root(), "demo").unwrap();
    let root = directory.path("demo");

    let (text, failed) = call(&root, "new_module", &json!({"name": "billing"}));
    let (refused, refused_failed) = call(&root, "new_module", &json!({"name": "billing"}));

    assert!(!failed, "{text}");
    assert!(text.contains("  wrote app/src/wiring.rs"), "{text}");
    assert!(text.ends_with("sofie check:\nok\n"), "{text}");
    assert!(root.join("modules/billing/src/module.rs").is_file());
    assert!(root.join("AGENTS.md").is_file());
    assert!(
        refused_failed && refused.contains("exists already"),
        "{refused}"
    );
}

#[test]
fn edit_screen_replaces_a_unique_text_and_dry_runs_it_on_a_copy() {
    let root = shop();
    let template = root.join("modules/order/src/areas/customer/screens/orders.html");
    let before = fs::read_to_string(&template).unwrap();
    let key = "order/customer/orders";

    let (dry, _) = call(
        &root,
        "edit_screen",
        &json!({"key": key, "find": "as=\"order\">", "replace": "as=\"order\" link=\"/account/orders/{order.id}\">", "dry_run": true}),
    );
    let (missing, missing_failed) = call(
        &root,
        "edit_screen",
        &json!({"key": key, "find": "<Nothing />", "replace": ""}),
    );
    let (twice, twice_failed) = call(
        &root,
        "edit_screen",
        &json!({"key": key, "find": "<Submit label=", "replace": ""}),
    );
    let (unknown, unknown_failed) = call(
        &root,
        "edit_screen",
        &json!({"key": "order/customer/nothing", "find": "a", "replace": "b"}),
    );

    assert_eq!(fs::read_to_string(&template).unwrap(), before);
    let path = "modules/order/src/areas/customer/screens/orders.html";
    for expected in [
        "dry run, nothing was written; it would:\n".to_owned(),
        format!("\n  change {path}\n"),
        format!("edited {path}\nsofie gen"),
        "\n+  <Table id=\"orders\" of={data} columns=\"id status total\" as=\"order\" \
         link=\"/account/orders/{order.id}\">\n"
            .to_owned(),
    ] {
        assert!(dry.contains(&expected), "missing {expected:?} in {dry}");
    }
    assert!(
        missing_failed && missing.starts_with(&format!("`find` is not in {path}")),
        "{missing}"
    );
    assert!(
        twice_failed && twice.starts_with("`find` occurs more than once"),
        "{twice}"
    );
    assert!(
        unknown_failed && unknown.starts_with("no screen template `order/customer/nothing`"),
        "{unknown}"
    );
}

#[test]
fn edit_screen_writes_the_template_then_runs_gen_and_check() {
    let directory = TemporaryDirectory::new("sofie-mcp-edit");
    sofie_cli::new_app(directory.root(), "demo").unwrap();
    let root = directory.path("demo");
    call(&root, "new_module", &json!({"name": "billing"}));
    let template = root.join("modules/billing/src/areas/web/screens/home.html");
    fs::create_dir_all(template.parent().unwrap()).unwrap();
    fs::write(&template, "<Section title=\"Home\">\n</Section>\n").unwrap();

    let (text, failed) = call(
        &root,
        "edit_screen",
        &json!({"key": "billing/web/home", "find": "Home", "replace": "Welcome"}),
    );

    assert_eq!(
        fs::read_to_string(&template).unwrap(),
        "<Section title=\"Welcome\">\n</Section>\n"
    );
    assert!(
        text.starts_with("edited modules/billing/src/areas/web/screens/home.html\nsofie gen"),
        "{text}"
    );
    assert!(!failed && text.ends_with("sofie check:\nok\n"), "{text}");
}

/// Each test names its own `package`: Cargo hashes a member by its relative path, so alike workspaces would share build output.
fn tiny_app(package: &str, library: &str) -> TemporaryDirectory {
    let directory = TemporaryDirectory::new("sofie-mcp-cargo");
    directory.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\"]\n",
    );
    directory.write("app/Cargo.toml", &manifest(package, ""));
    directory.write("app/src/lib.rs", library);
    directory.write("modules/.gitkeep", "");
    directory
}

#[test]
fn run_tools_answer_cargo_s_diagnostics_and_failures_only() {
    let app = tiny_app(
        "tested",
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds() {\n        assert_eq!(1 + 1, 2);\n    }\n\n    \
         #[test]\n    fn subtracts() {\n        assert_eq!(2 - 1, 0, \"two minus one\");\n    }\n}\n",
    );

    let initialized = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(app.root())
        .status()
        .unwrap();
    assert!(initialized.success());

    let (built, build_failed) = call(app.root(), "build", &json!({}));
    let (tested, test_failed) = call(app.root(), "test", &json!({}));
    let (filtered, filtered_failed) = call(app.root(), "test", &json!({"filter": "adds"}));
    let (unmatched, unmatched_failed) = call(app.root(), "test", &json!({"filter": "missing"}));

    assert_eq!(
        (built.as_str(), build_failed),
        ("cargo build succeeded\n", false)
    );
    assert!(test_failed, "{tested}");
    assert!(
        tested.starts_with("cargo test failed with exit code 101\npassed 1, failed 1, ignored 0\n"),
        "{tested}"
    );
    assert!(tested.contains("two minus one"), "{tested}");
    assert!(
        !tested.contains("Compiling") && !tested.contains("tests::adds ... ok"),
        "{tested}"
    );
    assert!(!filtered_failed, "{filtered}");
    assert!(filtered.contains("test result: ok. 1 passed"), "{filtered}");
    assert!(
        filtered.ends_with(
            "passed and ignored, by test file:\n  app/src/lib.rs (changed): 1 passed\n    \
             tests::adds\nthe filter `adds` left out 1 of the 2 tests in changed files:\n  \
             app/src/lib.rs: tests::subtracts"
        ),
        "{filtered}"
    );
    assert!(
        tested.ends_with(
            "failed (1):\n  tests::subtracts\npassed and ignored, by test file (every name in \
             changed files):\n  app/src/lib.rs (changed): 1 passed\n    tests::adds"
        ),
        "{tested}"
    );
    assert!(
        unmatched_failed && unmatched.contains("no test path contains `missing`, so none ran"),
        "{unmatched}"
    );
}

#[test]
fn the_tests_tool_lists_each_test_file_with_its_test_functions() {
    let app = tiny_app(
        "listed",
        "pub fn helper() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds() {}\n\n    \
         fn not_a_test() {}\n}\n",
    );
    app.write(
        "app/tests/orders.rs",
        "#[tokio::test]\nasync fn places_an_order() {}\n\n#[test_app]\nasync fn cancels(app: TestApp) {}\n",
    );
    app.write("app/tests/support/mod.rs", "pub fn sign_in() {}\n");

    let (text, failed) = call(app.root(), "tests", &json!({}));

    assert!(!failed, "{text}");
    assert_eq!(
        text,
        "app/src/lib.rs\n  #[test] tests::adds\n\napp/tests/orders.rs\n  #[tokio::test] \
         places_an_order\n  #[test_app] cancels"
    );

    let (source, source_failed) = call(app.root(), "tests", &json!({"name": "cancels"}));
    let (nested, _) = call(app.root(), "tests", &json!({"name": "adds"}));
    let (missing, missing_failed) = call(app.root(), "tests", &json!({"name": "helper"}));

    assert!(!source_failed, "{source}");
    assert_eq!(
        source,
        "app/tests/orders.rs:4-5\n#[test_app]\nasync fn cancels(app: TestApp) {}"
    );
    assert_eq!(nested, "app/src/lib.rs:5-6\n    #[test]\n    fn adds() {}");
    assert!(
        missing_failed && missing.contains("no test is named `helper`"),
        "{missing}"
    );
}

fn git(root: &Path, arguments: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=sofie",
            "-c",
            "user.email=sofie@example.test",
        ])
        .args(arguments)
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success(), "git {arguments:?}");
}

#[test]
fn changes_lists_the_app_s_changed_files_and_one_file_s_diff() {
    let app = tiny_app("changed", "pub fn one() {}\n");
    app.write("app/src/old.rs", "pub fn old() {}\n");
    let (outside_repository, outside_failed) = call(app.root(), "changes", &json!({}));
    git(app.root(), &["init", "--quiet"]);
    git(app.root(), &["add", "."]);
    git(app.root(), &["commit", "--quiet", "-m", "first"]);
    let (clean, _) = call(app.root(), "changes", &json!({}));
    app.write("app/src/lib.rs", "pub fn one() {}\npub fn two() {}\n");
    app.write("app/src/new.rs", "pub fn new() {}\n");
    fs::remove_file(app.path("app/src/old.rs")).unwrap();

    let (listed, listed_failed) = call(app.root(), "changes", &json!({}));
    let (diff, _) = call(app.root(), "changes", &json!({"path": "app/src/lib.rs"}));
    let (created, _) = call(app.root(), "changes", &json!({"path": "app/src/new.rs"}));
    let (outside, outside_path_failed) = call(app.root(), "changes", &json!({"path": "../x"}));

    assert!(
        outside_failed && outside_repository.contains("not in a git repository"),
        "{outside_repository}"
    );
    assert_eq!(clean, "no changes under the app root since the last commit");
    assert!(!listed_failed, "{listed}");
    assert_eq!(
        listed,
        "added (1):\n  app/src/new.rs\nmodified (1):\n  app/src/lib.rs\ndeleted (1):\n  \
         app/src/old.rs\n`changes` with `path` shows one file's diff"
    );
    assert!(diff.contains("+pub fn two() {}"), "{diff}");
    assert_eq!(created, "new file app/src/new.rs:\npub fn new() {}\n");
    assert!(outside_path_failed, "{outside}");
}

/// A stand-in for the facade, so `cargo metadata` lists its features without a build.
fn app_with_facade() -> TemporaryDirectory {
    let app = TemporaryDirectory::new("sofie-mcp-feature");
    app.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\"]\n",
    );
    app.write(
        "app/Cargo.toml",
        &manifest(
            "featured",
            "# The framework.\nsofie = { package = \"sofie-framework\", path = \"../facade\", \
             features = [\"engine\"] }\n",
        ),
    );
    app.write("app/src/lib.rs", "");
    app.write(
        "facade/Cargo.toml",
        "[package]\nname = \"sofie-framework\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [lib]\nname = \"sofie\"\n\n[features]\ndefault = []\nengine = []\ni18n = [\"engine\"]\n",
    );
    app.write("facade/src/lib.rs", "");
    app.write("modules/.gitkeep", "");
    app
}

#[test]
fn the_feature_tool_edits_the_sofie_dependency_s_features_against_the_facade_s_list() {
    let app = app_with_facade();
    let manifest_path = app.path("app/Cargo.toml");
    let before = fs::read_to_string(&manifest_path).unwrap();

    let (unknown, unknown_failed) = call(
        app.root(),
        "feature",
        &json!({"action": "add", "name": "i18m"}),
    );
    let (dry, _) = call(
        app.root(),
        "feature",
        &json!({"action": "add", "name": "i18n", "dry_run": true}),
    );
    let after_dry_run = fs::read_to_string(&manifest_path).unwrap();
    let edit = sofie_cli::change_feature(app.root(), "i18n", sofie_cli::FeatureChange::Add);

    assert!(unknown_failed, "{unknown}");
    assert!(
        unknown.contains("`sofie` has no feature `i18m`; its features are engine, i18n"),
        "{unknown}"
    );
    assert!(dry.contains("app/Cargo.toml"), "{dry}");
    assert_eq!(after_dry_run, before);
    assert_eq!(
        edit.unwrap().to_string(),
        format!(
            "{}: the `sofie` dependency added the feature `i18n`; its features are now [engine, \
             i18n]",
            manifest_path.display()
        )
    );
    assert_eq!(
        fs::read_to_string(&manifest_path).unwrap(),
        before.replace("[\"engine\"]", "[\"engine\", \"i18n\"]")
    );
}

#[test]
fn lint_lists_unformatted_files_and_clippy_s_findings_and_formats_with_write() {
    let app = tiny_app(
        "linted",
        "pub fn two( ) -> u8 {\n    let unused = 1;\n    2\n}\n",
    );

    let (checked, check_failed) = call(app.root(), "lint", &json!({}));
    let (written, _) = call(app.root(), "lint", &json!({"write": true}));

    assert!(check_failed, "{checked}");
    assert!(
        checked.starts_with(
            "cargo fmt --check: these files need formatting; `lint` with `write` formats \
             them:\n  app/src/lib.rs\ncargo clippy failed with exit code 101\n"
        ),
        "{checked}"
    );
    assert!(
        checked.contains("src/lib.rs:2:9: error: unused variable: `unused`"),
        "{checked}"
    );
    assert!(
        written.starts_with("cargo fmt formatted these files:\n  app/src/lib.rs\n"),
        "{written}"
    );
    assert!(
        fs::read_to_string(app.path("app/src/lib.rs"))
            .unwrap()
            .starts_with("pub fn two() -> u8 {"),
    );
}

#[test]
fn a_build_error_is_answered_in_short_form() {
    let app = tiny_app("broken", "pub fn broken() -> u8 {\n    missing\n}\n");

    let (text, failed) = call(app.root(), "build", &json!({}));

    assert!(failed, "{text}");
    assert!(text.contains("src/lib.rs:2:5: error[E0425]"), "{text}");
}

#[test]
#[ignore = "builds and runs the shop: needs DATABASE_URL and SOFIE_SIGNING_KEYS of a real Postgres"]
fn app_tools_run_the_shop_and_answer_a_request_with_its_span_tree() {
    let tool = |id: u64, name: &str, arguments: Value| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
               "params": {"name": name, "arguments": arguments}})
    };

    let answers = exchange(
        &shop(),
        &[
            tool(
                1,
                "request",
                json!({"path": "/account/orders", "session": "visitor"}),
            ),
            tool(
                2,
                "request",
                json!({"method": "post", "path": "/account/orders",
                                      "body": "product=1", "target": "orders"}),
            ),
            tool(3, "traces", json!({})),
            tool(4, "logs", json!({"filter": "request finished"})),
            tool(5, "dev", json!({"action": "status"})),
            tool(6, "dev", json!({"action": "stop"})),
        ],
    );

    let text = |index: usize| {
        answers[index]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
    };
    let page = text(0);
    assert!(page.starts_with("HTTP "), "{page}");
    assert!(page.contains("\nx-request-id: "), "{page}");
    let trace_id = page.split("\ntrace ").nth(1).unwrap().get(..32).unwrap();
    assert!(page.contains("\nrequest GET "), "{page}");
    assert!(
        !text(1).contains("HTTP 403"),
        "the Origin passes the CSRF check: {}",
        text(1)
    );
    assert!(text(1).contains("\nrequest POST "), "{}", text(1));
    assert!(text(2).contains(trace_id), "{}", text(2));
    assert!(text(3).contains("request finished"), "{}", text(3));
    assert!(text(3).contains("next cursor: "), "{}", text(3));
    assert!(text(4).starts_with("running: pid "), "{}", text(4));
    assert_eq!(text(5), "stopped the app");
}

#[test]
fn request_refuses_a_path_or_method_it_cannot_send_before_starting_the_app() {
    let directory = TemporaryDirectory::new("sofie-mcp-request");

    let (path, path_failed) = call(directory.root(), "request", &json!({"path": "orders"}));
    let (method, method_failed) = call(
        directory.root(),
        "request",
        &json!({"path": "/orders", "method": "GE T"}),
    );

    assert!(path_failed && path.contains("starts with `/`"), "{path}");
    assert!(
        method_failed && method.contains("is not an HTTP method"),
        "{method}"
    );
}

#[test]
fn dev_run_needs_the_schedule_it_queues() {
    let directory = TemporaryDirectory::new("sofie-mcp-schedule");

    let (text, failed) = call(directory.root(), "dev", &json!({"action": "run"}));

    assert!(failed && text.contains("`run` takes `schedule`"), "{text}");
}

#[test]
fn a_long_read_answer_is_cut() {
    let directory = TemporaryDirectory::new("sofie-mcp-long");
    sofie_cli::new_app(directory.root(), "demo").unwrap();
    let root = directory.path("demo");
    call(&root, "new_module", &json!({"name": "billing"}));
    for number in 1..=300 {
        let name = format!("{number:04}_{}.sql", "a".repeat(60));
        fs::write(root.join("modules/billing/migrations").join(name), "").unwrap();
    }

    let (text, _) = call(&root, "schema", &json!({}));

    assert!(text.len() < 17 * 1024, "{} bytes", text.len());
    assert!(text.contains("… cut at"), "{text}");
}

#[test]
fn every_tool_s_long_answer_is_cut() {
    let app = tiny_app("long_answer", "");

    let (text, failed) = call(
        app.root(),
        "new_module",
        &json!({"name": "a".repeat(20_000)}),
    );

    assert!(failed, "{text}");
    assert!(text.len() < 17 * 1024, "{} bytes", text.len());
    assert!(text.contains("… cut at"), "{text}");
}

/// An app that answers 200 to every request, beside a stub `sofie` carrying the `development`
/// feature `sofie mcp` builds with, so the test needs neither the framework nor a database.
#[cfg(all(unix, feature = "slow-tests"))]
fn answering_app() -> TemporaryDirectory {
    let directory = TemporaryDirectory::new("sofie-mcp-interrupt");
    directory.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"sofie\"]\n",
    );
    directory.write(
        "app/Cargo.toml",
        &manifest("interrupted", "sofie = { path = \"../sofie\" }\n"),
    );
    directory.write("app/src/main.rs", "fn main() {\n    sofie::answer();\n}\n");
    directory.write(
        "sofie/Cargo.toml",
        &format!("{}\n[features]\ndevelopment = []\n", manifest("sofie", "")),
    );
    // In the framework's stand-in, as `sofie check` refuses a socket in the app's own code.
    directory.write(
        "sofie/src/lib.rs",
        "use std::io::{Read, Write};\n\npub fn answer() {\n    \
         let address = std::env::var(\"SOFIE_ADDRESS\").unwrap();\n    \
         for stream in std::net::TcpListener::bind(address).unwrap().incoming() {\n        \
         let mut stream = stream.unwrap();\n        let _ = stream.read(&mut [0; 1024]);\n        \
         let _ = stream.write_all(b\"HTTP/1.1 200 OK\\r\\ncontent-length: 0\\r\\nconnection: \
         close\\r\\n\\r\\n\");\n    }\n}\n",
    );
    directory.write("modules/.gitkeep", "");
    directory.write(
        ".env",
        "DATABASE_URL=postgres://sofie@127.0.0.1:25432/never_opened\n",
    );
    directory
}

#[cfg(all(unix, feature = "slow-tests"))]
#[test]
fn sigterm_stops_sofie_mcp_and_reaps_the_app_it_started() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let app = answering_app();
    let mut mcp = Command::new(env!("CARGO_BIN_EXE_sofie"))
        .arg("mcp")
        .current_dir(app.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let start = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": "dev", "arguments": {"action": "start"}}});
    writeln!(mcp.stdin.as_mut().unwrap(), "{start}").unwrap();
    let mut answer = String::new();
    BufReader::new(mcp.stdout.as_mut().unwrap())
        .read_line(&mut answer)
        .unwrap();
    let answer: Value = serde_json::from_str(&answer).unwrap();
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    let pid = text
        .strip_prefix("running: pid ")
        .and_then(|rest| rest.split(',').next())
        .unwrap_or_else(|| panic!("{text}"))
        .to_owned();
    let signal = |name: &str, pid: &str| {
        Command::new("kill")
            .args([name, pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };

    assert!(signal("-TERM", &mcp.id().to_string()));

    assert_eq!(mcp.wait().unwrap().code(), Some(143));
    let deadline = Instant::now() + Duration::from_secs(5);
    while signal("-0", &pid) {
        assert!(Instant::now() < deadline, "the app (pid {pid}) still runs");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `sh` ignores SIGHUP the way `nohup` does, then becomes `sofie mcp`, keeping the pid.
#[cfg(unix)]
fn hung_up_mcp(shell_prefix: &str) -> (std::process::ExitStatus, bool) {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let mut mcp = Command::new("sh")
        .arg("-c")
        .arg(format!("{shell_prefix}exec \"$0\" mcp ."))
        .arg(env!("CARGO_BIN_EXE_sofie"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = mcp.stdin.take().unwrap();
    let mut stdout = BufReader::new(mcp.stdout.take().unwrap());
    let mut ping = || {
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})
        )?;
        let mut answer = String::new();
        stdout.read_line(&mut answer)?;
        Ok::<_, std::io::Error>(answer)
    };
    assert!(ping().unwrap().contains("\"result\""));

    let sent = Command::new("kill")
        .args(["-HUP", &mcp.id().to_string()])
        .status()
        .unwrap();

    assert!(sent.success());
    let answered_after = ping().is_ok_and(|answer| answer.contains("\"result\""));
    drop(ping);
    // The ping can be answered before the hangup lands; closing stdin then would end it cleanly.
    if !shell_prefix.is_empty() {
        drop(stdin);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = mcp.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            mcp.kill().unwrap();
            panic!("sofie mcp still runs 10 seconds after its hangup");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    (status, answered_after)
}

#[cfg(unix)]
#[test]
fn a_hangup_stops_sofie_mcp_unless_it_was_started_ignoring_hangups() {
    let (hung_up, _) = hung_up_mcp("");
    let (ignoring, answered_after) = hung_up_mcp("trap '' HUP; ");

    assert_eq!(hung_up.code(), Some(129));
    assert_eq!(ignoring.code(), Some(0));
    assert!(answered_after);
}
