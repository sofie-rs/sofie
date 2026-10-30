use std::process::Command;

fn built_for_development() -> bool {
    let output = Command::new(env!("CARGO_BIN_EXE_shop"))
        .args(["serve", "--at", "never"])
        .output()
        .unwrap();
    !String::from_utf8_lossy(&output.stderr).contains("is not a command")
}

/// `db fresh` drops every table, so only a `development` build (`sofie dev`,
/// `sofie db fresh`) has it.
#[test]
fn a_production_build_has_no_db_fresh() {
    if built_for_development() {
        eprintln!("skipped: this build of the shop has sofie's `development` feature");
        return;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_shop"))
        .args(["db", "fresh"])
        .env(
            "DATABASE_URL",
            "postgres://sofie:secret@db.example.com/shop",
        )
        .output()
        .unwrap();

    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(error.contains("`db fresh` is not a command"), "{error}");
    assert!(!error.contains("secret"), "{error}");
}

#[test]
fn an_unknown_role_is_refused_with_the_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_shop"))
        .args(["serve", "--role", "cron"])
        .output()
        .unwrap();

    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(error.contains("`cron` is not a role"), "{error}");
}
