use sofie_core::{Error, Result, is_local_database};
use sofie_db::{Db, Migrations};

/// Only a database on this machine is touched: what it drops cannot be brought back.
pub(crate) async fn fresh_database(
    database_url: &str,
    db: &Db,
    migrations: &[Migrations],
) -> Result<()> {
    if !is_local_database(database_url) {
        return Err(Error::internal_message(
            "`db fresh` drops every table of the app, so it runs only on a database on this \
             machine (localhost, 127.0.0.1, ::1 or a socket), and DATABASE_URL names another host",
        ));
    }
    db.schema().fresh(migrations).await
}
