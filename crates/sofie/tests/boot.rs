use std::process::Command;

use sofie::db::Db;
use sofie::db::worker::Worker;
use sofie::http::App;

const PROBE: &str = "SOFIE_BOOT_PROBE";

async fn app(db: Db) -> sofie::core::Result<App> {
    Ok(App::new(db))
}

/// Run by `a_failed_boot_prints_its_message_and_exits_non_zero` in a process of its own.
#[test]
fn boot_probe() {
    if std::env::var_os(PROBE).is_none() {
        return;
    }
    let _ = sofie::run(&[], app, Worker::new);
}

#[test]
fn a_failed_boot_prints_its_message_and_exits_non_zero() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "boot_probe", "--nocapture", "--test-threads=1"])
        .env(PROBE, "1")
        .env_remove("DATABASE_URL")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("Error: "), "{stderr}");
    assert!(!stderr.contains(" {"), "printed as Debug: {stderr}");
}
