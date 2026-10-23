use std::path::Path;

use sofie_model::naming::crate_name;

use super::{Finding, Rule};

const DRIVER_CRATES: [&str; 9] = [
    "tokio-postgres",
    "postgres",
    "deadpool-postgres",
    "sqlx",
    "diesel",
    "sea-orm",
    "rusqlite",
    "mysql",
    "mysql_async",
];

pub(super) fn is_driver_crate(package: &str) -> bool {
    DRIVER_CRATES.contains(&package)
}

pub(super) fn names_driver(segments: &[String]) -> bool {
    segments.first().is_some_and(|root| {
        DRIVER_CRATES
            .iter()
            .any(|package| crate_name(package) == *root)
    })
}

pub(super) fn finding(file: &Path, line: usize, written: &str) -> Finding {
    Finding {
        rule: Rule::DatabaseDriver,
        file: file.to_path_buf(),
        line,
        message: format!(
            "`{written}` reaches the database around the framework, which bypasses the owner \
             scoping, the unit of work and the database seam every module's data goes through"
        ),
        fix: "read through `Read` and read models, and write by returning aggregates from an \
              action"
            .to_owned(),
    }
}
