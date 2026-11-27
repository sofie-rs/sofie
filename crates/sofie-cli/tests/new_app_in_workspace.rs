use std::fs;
use std::path::Path;

use sofie_cli::{new_app, new_module};
use sofie_model::testing::{TemporaryDirectory, manifest};

/// A workspace of its own crates, declaring the framework but not the CLI.
fn enclosing_workspace() -> TemporaryDirectory {
    let directory = TemporaryDirectory::new("sofie-new-app-in-workspace");
    let framework = Path::new(env!("CARGO_MANIFEST_DIR")).join("../sofie");
    directory.write(
        "Cargo.toml",
        &format!(
            "[workspace]\nresolver = \"3\"\nmembers = [\n    \"tools/lint\",\n]\n\n\
             [workspace.package]\nedition = \"2024\"\nversion = \"0.1.0\"\npublish = false\n\n\
             [workspace.dependencies]\nsofie = {{ package = \"sofie-framework\", path = \"{}\" }}\n\n\
             [workspace.lints.rust]\nunsafe_code = \"forbid\"\n",
            framework.display()
        ),
    );
    directory.write("tools/lint/Cargo.toml", &manifest("lint", ""));
    directory.write("tools/lint/src/lib.rs", "");
    fs::create_dir_all(directory.path("examples")).unwrap();
    directory
}

#[test]
fn an_app_inside_a_workspace_joins_it_and_a_module_adds_its_contract_there() {
    let directory = enclosing_workspace();
    let root = directory.root().canonicalize().unwrap();

    new_app(&root.join("examples"), "demo").unwrap();
    new_module(&root.join("examples/demo"), "billing").unwrap();

    let workspace = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(
        workspace.contains(
            "members = [\n    \"tools/lint\",\n    \"examples/demo/app\",\n    \
             \"examples/demo/modules/*\",\n    \"examples/demo/modules/*/contract\",\n]"
        ),
        "{workspace}"
    );
    let cli = workspace
        .lines()
        .find(|line| line.starts_with("sofie-cli = "))
        .unwrap_or_else(|| panic!("the CLI the app builds with is declared: {workspace}"));
    assert!(
        cli.starts_with("sofie-cli = { path = \"../") && cli.ends_with("crates/sofie-cli\" }"),
        "{cli}"
    );
    assert_eq!(workspace.matches("sofie = ").count(), 1, "{workspace}");
    for own in ["Cargo.toml", "Cargo.lock", ".gitignore"] {
        assert!(!root.join("examples/demo").join(own).exists(), "{own}");
    }
    let app = fs::read_to_string(root.join("examples/demo/app/Cargo.toml")).unwrap();
    for line in [
        "edition.workspace = true",
        "sofie.workspace = true",
        "sofie-cli.workspace = true",
        "[lints]",
    ] {
        assert!(app.contains(line), "{line}: {app}");
    }
    assert!(
        root.join("examples/demo/modules/billing/contract/src/lib.rs")
            .is_file()
    );
}
