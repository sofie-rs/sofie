#![cfg(feature = "slow-tests")]

use std::path::Path;
use std::process::Command;

#[test]
fn the_shop_stays_within_its_limits_against_loco() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/limits.sh");

    let output = Command::new("bash")
        .arg(&script)
        .output()
        .expect("bash runs bench/limits.sh");

    let table = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "a limit in bench/loco-baseline.json is broken:\n{table}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
