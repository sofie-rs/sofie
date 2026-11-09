#![cfg(feature = "slow-tests")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sofie_auth::{SIGNING_KEYS, SigningKeys};
use sofie_testing::TestDatabase;

fn shop() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shop")
}

/// On a database of its own: the shop's `.env` names the developer's, which a fresh Postgres lacks.
fn sofie_on_shop(arguments: &[&str]) -> String {
    let database = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(TestDatabase::create());
    let output: Output = Command::new(env!("CARGO_BIN_EXE_sofie"))
        .args(arguments)
        .arg(shop())
        .env("DATABASE_URL", database.url())
        .env(SIGNING_KEYS, SigningKeys::generate_text().unwrap())
        .output()
        .unwrap();
    drop(database);
    sofie_testing::finish_teardown();
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    text
}

#[test]
fn fuzzing_the_shop_reaches_its_handlers_and_finds_no_500() {
    let report = sofie_on_shop(&["fuzz", "--seed", "1", "--requests", "80"]);

    assert!(
        report.starts_with("sofie fuzz: 80 requests to ")
            && report.contains(", seed 1: no 500, panic or foreign record reached\n"),
        "{report}"
    );
    let answered = report
        .lines()
        .find_map(|line| line.strip_prefix("  answered "))
        .unwrap_or_else(|| panic!("{report}"));
    let statuses: Vec<&str> = answered.split(", ").collect();
    assert!(
        statuses.iter().any(|status| status.starts_with('2'))
            && statuses.iter().any(|status| status.starts_with('4')),
        "{report}"
    );
}

#[test]
fn seeding_the_shop_posts_its_seed_file_and_the_volume_to_each_creating_route() {
    let report = sofie_on_shop(&["seed", "--volume", "5"]);

    assert_eq!(report, "order.customer.place_order: 7 created\n");
}
