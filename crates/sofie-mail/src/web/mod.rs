//! What reaches sofie-mail over HTTP: unsubscribes from bulk mail and the mail provider's
//! reports of bounces and complaints, each suppressing an address.

mod events;
mod page;

use sofie_core::Result;
use sofie_db::{Outcome, Plan, Stamp};
use sofie_http::{Routes, Web};

pub use events::Events;

use crate::suppression::Suppression;

pub fn routes(routes: &mut Routes<Web>, events: &Events) {
    page::routes(routes);
    events.routes(routes);
}

pub(crate) struct Suppressed(pub(crate) Option<Suppression>);

impl Outcome for Suppressed {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.0.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.0.committed(stamp);
    }
}
