use std::fmt::Write;

use sofie_core::{Error, Result};
use sofie_db::{Db, Migrations, Mismatch};

pub(crate) async fn print(db: &Db, migrations: &[Migrations]) -> Result<()> {
    let mismatches = db.schema().check(migrations).await?;
    print!("{}", report(&mismatches));
    if mismatches.is_empty() {
        return Ok(());
    }
    Err(Error::internal_message(format!(
        "{} disagreement(s) between the database and the schema; add a migration that fixes them",
        mismatches.len()
    )))
}

fn report(mismatches: &[Mismatch]) -> String {
    if mismatches.is_empty() {
        return "every table matches its schema\n".to_owned();
    }
    let mut text = String::new();
    for mismatch in mismatches {
        let _ = writeln!(text, "{mismatch}");
    }
    text
}

#[cfg(test)]
mod tests {
    use sofie_db::{Mismatch, MismatchKind};

    use super::report;

    #[test]
    fn each_mismatch_is_a_line_naming_its_table() {
        let mismatches = [
            Mismatch {
                table: "order.order_line".to_owned(),
                kind: MismatchKind::MissingTable,
            },
            Mismatch {
                table: "order.orders".to_owned(),
                kind: MismatchKind::NotNull {
                    column: "note".to_owned(),
                },
            },
        ];

        assert_eq!(
            report(&mismatches),
            "order.order_line: the table does not exist\n\
             order.orders: column `note` is NOT NULL, but the schema's field is optional\n"
        );
        assert_eq!(report(&[]), "every table matches its schema\n");
    }
}
