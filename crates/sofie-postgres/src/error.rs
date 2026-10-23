use sofie_core::Error;
use sofie_db::UniqueViolation;
use tokio_postgres::error::SqlState;

/// Every error the driver returns passes here. Detail and hint can quote row values, so only
/// severity, code, message and a unique violation's index are kept.
pub(crate) fn classified(doing: &str, error: tokio_postgres::Error) -> Error {
    let Some(server) = error.as_db_error() else {
        return Error::internal(doing.to_owned(), error);
    };
    let code = server.code();
    if [
        SqlState::T_R_SERIALIZATION_FAILURE,
        SqlState::T_R_DEADLOCK_DETECTED,
    ]
    .contains(code)
    {
        return Error::Conflict;
    }
    if [
        SqlState::UNTRANSLATABLE_CHARACTER,
        SqlState::CHARACTER_NOT_IN_REPERTOIRE,
    ]
    .contains(code)
    {
        return Error::BadRequest(
            "the text holds a character the database cannot store, such as NUL".to_owned(),
        );
    }
    let context = format!(
        "{doing}: {} {}: {}",
        server.severity(),
        code.code(),
        server.message()
    );
    if *code == SqlState::FOREIGN_KEY_VIOLATION {
        return Error::Rule(sofie_db::REFERENCE_REFUSAL.to_owned());
    }
    match server.constraint() {
        Some(index) if *code == SqlState::UNIQUE_VIOLATION => Error::internal(
            context,
            UniqueViolation {
                index: index.to_owned(),
            },
        ),
        Some(constraint) if *code == SqlState::EXCLUSION_VIOLATION => {
            Error::Rule(sofie_db::exclusion_refusal(constraint))
        }
        Some(constraint) if *code == SqlState::CHECK_VIOLATION => {
            Error::Rule(sofie_db::check_refusal(constraint))
        }
        _ => Error::internal_message(context),
    }
}

pub(crate) fn statement_error(sql: &str, error: tokio_postgres::Error) -> Error {
    classified(&format!("running `{sql}`"), error)
}
