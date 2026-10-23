//! Queued by a handler, delivered by the worker.

#[cfg(feature = "mail-web")]
pub use sofie_mail::web;
pub use sofie_mail::{
    Address, Body, Mail, Mailer, Reason, Scope, Suppression, Template, UNSUBSCRIBE_PATH, scopes,
    suppress, templates,
};

use sofie_core::Result;
use sofie_db::Db;
use sofie_mail::Setup;

/// Read once from `SOFIE_MAIL_*`, so every way the app boots (serving, a task) can send mail.
pub(crate) fn set_up(db: &Db) -> Result<Setup> {
    let setup = Setup::from_environment()?;
    sofie_mail::install(db, &setup);
    Ok(setup)
}
