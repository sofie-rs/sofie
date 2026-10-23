//! Mail rendered from templates, queued with a handler's commit and sent by
//! a job, so a request never waits on a mail server.

mod delivery;
mod mail;
mod settings;
mod smtp;
mod suppression;
mod templates;
#[cfg(feature = "testing")]
pub mod testing;
mod text;
mod transport;
mod unsubscribe;
#[cfg(feature = "web")]
pub mod web;

use std::sync::Arc;

use sofie_core::{Error, Result, Show};
use sofie_db::worker::Worker;
use sofie_db::{Db, Dep, Extensions, Jobs, Queued, Resolve, Writes};

pub use mail::{Address, Body, Mail};
pub use settings::{Security, SmtpSettings};
pub use suppression::{Reason, Scope, Suppression, scopes, suppress};
pub use templates::{Template, templates};
pub use unsubscribe::PATH as UNSUBSCRIBE_PATH;

use settings::{Chosen, Settings};
use templates::Registered;
use transport::Transport;

/// What every mail is sent with: `sofie::run` reads it from `SOFIE_MAIL_*`, sofie-testing makes a fake.
#[derive(Clone)]
pub struct Setup {
    from: Address,
    base_url: Option<String>,
    transport: Arc<dyn Transport>,
}

impl Setup {
    #[doc(hidden)]
    pub fn from_environment() -> Result<Self> {
        let settings = Settings::read(sofie_core::environment_variable)?;
        let transport: Arc<dyn Transport> = match settings.transport {
            Chosen::Smtp(smtp) => {
                Arc::new(smtp::Smtp::new(smtp, settings.from.domain().to_owned()))
            }
            Chosen::Log { preview } => Arc::new(transport::Logged { preview }),
        };
        Ok(Self {
            from: settings.from,
            base_url: settings.base_url,
            transport,
        })
    }
}

#[doc(hidden)]
pub fn install(db: &Db, setup: &Setup) {
    sofie_db::provide(db, Arc::new(setup.clone()));
    sofie_db::name_job(db, delivery::JOB, delivery::deliver);
}

#[doc(hidden)]
#[must_use]
pub fn worker(worker: Worker) -> Worker {
    worker.job(delivery::JOB, delivery::deliver)
}

/// Only resolved as an argument: a handler's mails are sent once its outcome commits.
pub struct Mailer {
    setup: Setup,
    registered: Vec<Arc<Registered>>,
    jobs: Jobs,
}

impl Mailer {
    /// The app's template at `src/mails/<name>.html` of a module, keyed `<module>/<name>`.
    pub fn body(&self, key: &str, data: &(impl Show + ?Sized)) -> Result<Body> {
        let template = templates::find(&self.registered, key).ok_or_else(|| {
            Error::internal_message(format!(
                "there is no mail template `{key}`; write it as `src/mails/<name>.html` \
                 in its module and run `sofie gen`"
            ))
        })?;
        template.render(data)
    }

    /// A crate's own template, unless the app replaces it with one of the same key.
    pub fn body_from(&self, template: &Template, data: &(impl Show + ?Sized)) -> Result<Body> {
        templates::find(&self.registered, template.key())
            .unwrap_or(template)
            .render(data)
    }

    pub fn link(&self, path: &str) -> Result<String> {
        let base = self.setup.base_url.as_deref().ok_or_else(|| {
            settings::missing_base_url("links in mails start with the app's public URL")
        })?;
        if !path.starts_with('/') || path.starts_with("//") {
            return Err(Error::internal_message(format!(
                "a link in a mail names a path of this app, not `{path}`"
            )));
        }
        Ok(format!("{base}{path}"))
    }

    /// Queued with the handler's outcome; from `SOFIE_MAIL_FROM` unless the mail names a sender.
    pub fn send(&self, mail: Mail) -> Result<Queued> {
        let mail = mail.with_default_sender(&self.setup.from).checked()?;
        self.jobs.queue(delivery::deliver, &mail)
    }
}

impl<C: Writes> Resolve<C> for Mailer {
    async fn resolve(context: &mut C) -> Result<Self> {
        let setup = installed(context).await?;
        let registered = Extensions::<Registered>::resolve(context).await?;
        Ok(Self {
            setup,
            registered: registered.to_vec(),
            jobs: Jobs::resolve(context).await?,
        })
    }
}

async fn installed<C: Writes>(context: &mut C) -> Result<Setup> {
    let setup = Dep::<Setup>::resolve(context).await.map_err(|_| {
        Error::internal_message(
            "mail is not set up: `sofie::run` sets it up from `SOFIE_MAIL_*` with sofie's `mail` \
             feature, and sofie-testing with its own `mail` feature",
        )
    })?;
    Ok(Setup::clone(&setup))
}
