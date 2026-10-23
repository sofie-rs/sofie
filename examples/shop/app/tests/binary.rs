use std::process::Command;

/// `db fresh` drops every table, so only a `development` build (`sofie dev`,
/// `sofie db fresh`) has it.
#[test]
fn a_production_build_has_no_db_fresh() {
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
