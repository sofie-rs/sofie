use sofie::core::Uuid;
use sofie::prelude::*;

use crate::WebhookEvent;

pub(crate) const EVERY_EVENT: &str = "*";
const MAXIMUM_URL: usize = 2048;
const MAXIMUM_DESCRIPTION: usize = 500;
/// The previous secret signs too for this long after a rotation, so receivers can switch.
const OVERLAP: std::time::Duration = std::time::Duration::from_hours(24);

/// Where an owner hears events; its secret is kept sealed.
#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete))]
pub struct WebhookEndpoint {
    meta: Meta,
    #[owner]
    owner_id: Id<WebhookOwner>,
    url: String,
    description: Option<String>,
    /// Space-separated names, or `*` for every event.
    events: String,
    secret: String,
    previous_secret: Option<String>,
    previous_until: Option<Instant>,
    disabled_at: Option<Instant>,
    disabled_reason: Option<String>,
    succeeded_at: Option<Instant>,
}

pub type WebhookEndpointId = Id<WebhookEndpoint>;

/// A principal of the area the routes are mounted in, or the organization it acts for.
pub enum WebhookOwner {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disabled {
    ByOwner,
    /// Every delivery failed for days.
    Failing,
}

impl Disabled {
    fn as_str(self) -> &'static str {
        match self {
            Self::ByOwner => "by_owner",
            Self::Failing => "failing",
        }
    }
}

impl WebhookEndpoint {
    pub(crate) fn new(owner: Uuid, url: String, events: String, secret: String) -> Result<Self> {
        Ok(Self {
            meta: Meta::new()?,
            owner_id: Id::from_uuid(owner),
            url,
            description: None,
            events,
            secret,
            previous_secret: None,
            previous_until: None,
            disabled_at: None,
            disabled_reason: None,
            succeeded_at: None,
        })
    }

    #[must_use]
    pub fn owner(&self) -> Uuid {
        self.owner_id.uuid()
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn events(&self) -> impl Iterator<Item = &str> {
        self.events.split(' ')
    }

    #[must_use]
    pub fn disabled_at(&self) -> Option<Instant> {
        self.disabled_at
    }

    #[must_use]
    pub fn disabled_reason(&self) -> Option<&str> {
        self.disabled_reason.as_deref()
    }

    #[must_use]
    pub fn succeeded_at(&self) -> Option<Instant> {
        self.succeeded_at
    }

    pub(crate) fn subscribes_to(&self, event: &str) -> bool {
        self.disabled_at.is_none()
            && self
                .events()
                .any(|name| name == EVERY_EVENT || name == event)
    }

    pub(crate) fn sealed_secrets(&self, now: Instant) -> impl Iterator<Item = &str> {
        let previous = self
            .previous_secret
            .as_deref()
            .filter(|_| self.previous_until.is_some_and(|until| now < until));
        std::iter::once(self.secret.as_str()).chain(previous)
    }

    pub(crate) fn change_url(&mut self, url: String) {
        self.url = url;
    }

    pub(crate) fn change_events(&mut self, events: String) {
        self.events = events;
    }

    pub(crate) fn change_description(&mut self, description: Option<String>) {
        self.description = description;
    }

    pub(crate) fn rotate(&mut self, sealed: String, now: Instant) -> Result<()> {
        let until = now.checked_add(OVERLAP)?;
        self.previous_secret = Some(std::mem::replace(&mut self.secret, sealed));
        self.previous_until = Some(until);
        Ok(())
    }

    pub(crate) fn disable(&mut self, why: Disabled, now: Instant) {
        self.disabled_at = Some(now);
        self.disabled_reason = Some(why.as_str().to_owned());
    }

    pub(crate) fn enable(&mut self) {
        self.disabled_at = None;
        self.disabled_reason = None;
    }

    pub(crate) fn succeeded(&mut self, now: Instant) {
        self.succeeded_at = Some(now);
    }

    /// Since it last succeeded, or since it was made when it never did.
    pub(crate) fn failing_since(&self) -> Option<Instant> {
        self.succeeded_at.or_else(|| self.created_at())
    }
}

/// A receiver's own server, reached over TLS; whether its address is public is checked as each
/// delivery leaves.
pub(crate) fn checked_url(url: &str) -> Result<String> {
    let url = url.trim();
    let malformed = !url.starts_with("https://")
        || url.len() > MAXIMUM_URL
        || url.contains(|character: char| character.is_whitespace() || character.is_control());
    if malformed {
        return Err(Error::invalid("url", "must be an https URL"));
    }
    Ok(url.to_owned())
}

pub(crate) fn checked_description(description: Option<String>) -> Result<Option<String>> {
    let description = description
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    if description
        .as_ref()
        .is_some_and(|text| text.chars().count() > MAXIMUM_DESCRIPTION)
    {
        return Err(Error::invalid("description", "is too long"));
    }
    Ok(description)
}

pub(crate) fn checked_events(names: &[String], offered: &[WebhookEvent]) -> Result<String> {
    if names.is_empty() {
        return Err(Error::invalid("events", "names no event"));
    }
    if names.iter().any(|name| name == EVERY_EVENT) {
        return Ok(EVERY_EVENT.to_owned());
    }
    if let Some(unknown) = names
        .iter()
        .find(|name| !offered.iter().any(|event| event.name == name.as_str()))
    {
        return Err(Error::invalid(
            "events",
            format!("has no event `{unknown}`"),
        ));
    }
    let mut names = names.to_vec();
    names.sort();
    names.dedup();
    Ok(names.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAID: WebhookEvent = WebhookEvent::new("order.paid", "An order was paid.");
    const REFUNDED: WebhookEvent = WebhookEvent::new("order.refunded", "An order was refunded.");

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn subscriptions_name_offered_events_or_all_of_them() {
        let offered = [PAID, REFUNDED];

        assert_eq!(
            checked_events(
                &names(&["order.refunded", "order.paid", "order.paid"]),
                &offered
            )
            .unwrap(),
            "order.paid order.refunded"
        );
        assert_eq!(
            checked_events(&names(&["order.paid", "*"]), &offered).unwrap(),
            "*"
        );
        assert!(checked_events(&names(&["order.lost"]), &offered).is_err());
        assert!(checked_events(&[], &offered).is_err());
    }

    #[test]
    fn only_an_https_url_is_an_endpoint() {
        assert_eq!(
            checked_url(" https://hooks.example.com/in ").unwrap(),
            "https://hooks.example.com/in"
        );
        assert!(checked_url("http://hooks.example.com/in").is_err());
        assert!(checked_url("https://hooks.example.com/a b").is_err());
    }

    #[test]
    fn a_disabled_endpoint_hears_nothing_and_a_wildcard_hears_everything() {
        let mut endpoint = WebhookEndpoint::new(
            Uuid::now_v7().unwrap(),
            "https://hooks.example.com".to_owned(),
            "*".to_owned(),
            "sealed".to_owned(),
        )
        .unwrap();

        assert!(endpoint.subscribes_to("anything.at_all"));
        endpoint.disable(
            Disabled::ByOwner,
            Instant::from(sofie::core::Timestamp::UNIX_EPOCH),
        );
        assert!(!endpoint.subscribes_to("anything.at_all"));
    }

    #[test]
    fn a_rotated_secret_still_signs_for_a_day() {
        let mut endpoint = WebhookEndpoint::new(
            Uuid::now_v7().unwrap(),
            "https://hooks.example.com".to_owned(),
            "*".to_owned(),
            "first".to_owned(),
        )
        .unwrap();
        let now = Instant::from(sofie::core::Timestamp::from_second(1_800_000_000).unwrap());

        endpoint.rotate("second".to_owned(), now).unwrap();

        let within = now
            .checked_add(std::time::Duration::from_hours(23))
            .unwrap();
        let after = now
            .checked_add(std::time::Duration::from_hours(25))
            .unwrap();
        assert_eq!(
            endpoint.sealed_secrets(within).collect::<Vec<_>>(),
            ["second", "first"]
        );
        assert_eq!(
            endpoint.sealed_secrets(after).collect::<Vec<_>>(),
            ["second"]
        );
    }
}
