//! An owner's endpoints and their deliveries, as JSON, in the area whose principals own them;
//! the area's scope keeps each owner to its own.

use serde::Serialize;
use sofie::db::{Outcome, Plan, Queued, ReadOnly, Reply, Stamp};
use sofie::http::{ListedRoute, Query, Rate, Sealing};
use sofie::prelude::*;

use crate::delivery::WebhookDelivery;
use crate::endpoint::{
    Disabled, WebhookEndpoint, WebhookEndpointId, checked_description, checked_events, checked_url,
};
use crate::{WebhookArea, WebhookEvent, jobs, secret};

/// Each endpoint is a fan-out target for every event, so an owner keeps a few.
const MAXIMUM_ENDPOINTS: usize = 20;
const LISTED_DELIVERIES: u64 = 100;
/// Per principal: each sends a request to a URL the owner chose.
const SENDING: Rate = Rate::per_minute(10);
const TEST: WebhookEvent =
    WebhookEvent::new("webhook.test", "Sent when the owner tests an endpoint.");

#[rustfmt::skip]
pub const ROUTES: &[ListedRoute] = &[
    ListedRoute::new("routes", "GET", "/webhooks/events", "list_webhook_events"),
    ListedRoute::new("routes", "GET", "/webhooks", "list_webhook_endpoints"),
    ListedRoute::new("routes", "POST", "/webhooks", "create_webhook_endpoint"),
    ListedRoute::new("routes", "PATCH", "/webhooks/{webhook_endpoint}", "change_webhook_endpoint"),
    ListedRoute::new("routes", "DELETE", "/webhooks/{webhook_endpoint}", "delete_webhook_endpoint"),
    ListedRoute::new("routes", "POST", "/webhooks/{webhook_endpoint}/secret", "rotate_webhook_secret"),
    ListedRoute::new("routes", "POST", "/webhooks/{webhook_endpoint}/test", "test_webhook_endpoint"),
    ListedRoute::new("routes", "GET", "/webhooks/deliveries", "list_webhook_deliveries"),
    ListedRoute::new("routes", "POST", "/webhooks/deliveries/{webhook_delivery}/resend", "resend_webhook_delivery"),
];

pub fn routes<A: WebhookArea>(routes: &mut Routes<A>) {
    routes
        .get("/webhooks/events", list_events::<A>)
        .name("list_webhook_events");
    routes
        .get("/webhooks", list_endpoints)
        .name("list_webhook_endpoints");
    routes
        .post("/webhooks", create::<A>)
        .name("create_webhook_endpoint");
    routes
        .patch("/webhooks/{webhook_endpoint}", change::<A>)
        .name("change_webhook_endpoint");
    routes
        .delete("/webhooks/{webhook_endpoint}", delete)
        .name("delete_webhook_endpoint");
    routes
        .post("/webhooks/{webhook_endpoint}/secret", rotate)
        .name("rotate_webhook_secret");
    routes
        .post("/webhooks/{webhook_endpoint}/test", test)
        .name("test_webhook_endpoint")
        .throttle(SENDING);
    routes
        .get("/webhooks/deliveries", list_deliveries)
        .name("list_webhook_deliveries");
    routes
        .post("/webhooks/deliveries/{webhook_delivery}/resend", resend)
        .name("resend_webhook_delivery")
        .throttle(SENDING);
}

async fn list_events<A: WebhookArea>() -> Result<EventList> {
    Ok(EventList(A::WEBHOOK_EVENTS))
}

async fn list_endpoints(read: Read) -> Result<EndpointList> {
    let endpoints = WebhookEndpoint::query().latest().all(&read).await?;
    Ok(EndpointList(endpoints))
}

#[derive(Input)]
struct NewWebhookEndpoint {
    url: String,
    events: Vec<String>,
    description: Option<String>,
}

async fn create<A: WebhookArea>(
    me: Principal<A>,
    input: NewWebhookEndpoint,
    read: Read,
    sealing: Sealing,
) -> Result<WithSecret> {
    let url = checked_url(&input.url)?;
    let events = checked_events(&input.events, A::WEBHOOK_EVENTS)?;
    let description = checked_description(input.description)?;
    let owner = me.id().uuid();
    let held = WebhookEndpoint::query()
        .where_(WebhookEndpoint::OWNER_ID.eq(Id::from_uuid(owner)))
        .all(&read)
        .await?;
    if held.len() >= MAXIMUM_ENDPOINTS {
        return Err(Error::invalid(
            "url",
            format!("an owner keeps at most {MAXIMUM_ENDPOINTS} endpoints"),
        ));
    }
    let plain = secret::generate()?;
    let sealed = sealing.seal(secret::PURPOSE, plain.as_bytes())?;
    let mut endpoint = WebhookEndpoint::new(owner, url, events, sealed)?;
    endpoint.change_description(description);
    Ok(WithSecret {
        endpoint,
        secret: plain,
    })
}

#[derive(Input)]
struct WebhookEndpointChange {
    url: Option<String>,
    events: Option<Vec<String>>,
    /// Blank clears it.
    description: Option<String>,
    enabled: Option<bool>,
}

async fn change<A: WebhookArea>(
    mut endpoint: WebhookEndpoint,
    input: WebhookEndpointChange,
    clock: Clock,
) -> Result<Shown> {
    if let Some(url) = input.url {
        endpoint.change_url(checked_url(&url)?);
    }
    if let Some(events) = input.events {
        endpoint.change_events(checked_events(&events, A::WEBHOOK_EVENTS)?);
    }
    if let Some(description) = input.description {
        endpoint.change_description(checked_description(Some(description))?);
    }
    match input.enabled {
        Some(true) => endpoint.enable(),
        Some(false) if endpoint.disabled_at().is_none() => {
            endpoint.disable(Disabled::ByOwner, clock.now());
        }
        Some(false) | None => {}
    }
    Ok(Shown(endpoint))
}

/// Deliveries still waiting are given up when their turn comes.
async fn delete(mut endpoint: WebhookEndpoint) -> Result<WebhookEndpoint> {
    endpoint.delete();
    Ok(endpoint)
}

async fn rotate(
    mut endpoint: WebhookEndpoint,
    sealing: Sealing,
    clock: Clock,
) -> Result<WithSecret> {
    let plain = secret::generate()?;
    let sealed = sealing.seal(secret::PURPOSE, plain.as_bytes())?;
    endpoint.rotate(sealed, clock.now())?;
    Ok(WithSecret {
        endpoint,
        secret: plain,
    })
}

async fn test(endpoint: WebhookEndpoint, clock: Clock) -> Result<Queuing> {
    let now = clock.now();
    let body = sofie::serde_json::json!({
        "type": TEST.name,
        "timestamp": now.to_string(),
        "data": { "endpoint": endpoint.id() },
    })
    .to_string();
    let delivery = WebhookDelivery::new(endpoint.owner(), endpoint.id(), TEST.name, body)?;
    let queued = jobs::queue_delivery(&delivery, now)?;
    Ok(Queuing { delivery, queued })
}

#[derive(Input)]
struct DeliveryFilter {
    endpoint: Option<WebhookEndpointId>,
}

async fn list_deliveries(read: Read, filter: Query<DeliveryFilter>) -> Result<DeliveryList> {
    let mut query = WebhookDelivery::query().latest().limit(LISTED_DELIVERIES);
    if let Some(endpoint) = filter.0.endpoint {
        query = query.where_(WebhookDelivery::ENDPOINT.eq(endpoint));
    }
    Ok(DeliveryList(query.all(&read).await?))
}

async fn resend(mut delivery: WebhookDelivery, clock: Clock) -> Result<Queuing> {
    let now = clock.now();
    delivery.resend(now);
    let queued = jobs::queue_delivery(&delivery, now)?;
    Ok(Queuing { delivery, queued })
}

#[derive(Serialize)]
struct EndpointView {
    id: String,
    url: String,
    description: Option<String>,
    events: Vec<String>,
    disabled_at: Option<Instant>,
    disabled_reason: Option<String>,
    succeeded_at: Option<Instant>,
    created_at: Option<Instant>,
}

impl EndpointView {
    fn of(endpoint: &WebhookEndpoint) -> Self {
        Self {
            id: endpoint.id().to_string(),
            url: endpoint.url().to_owned(),
            description: endpoint.description().map(str::to_owned),
            events: endpoint.events().map(str::to_owned).collect(),
            disabled_at: endpoint.disabled_at(),
            disabled_reason: endpoint.disabled_reason().map(str::to_owned),
            succeeded_at: endpoint.succeeded_at(),
            created_at: endpoint.created_at(),
        }
    }
}

struct Shown(WebhookEndpoint);

impl Outcome for Shown {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.0.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.0.committed(stamp);
    }

    fn reply(self) -> Result<Reply> {
        Ok(Reply::data(EndpointView::of(&self.0)))
    }
}

/// The only answer that carries the secret; it is never shown again.
struct WithSecret {
    endpoint: WebhookEndpoint,
    secret: String,
}

#[derive(Serialize)]
struct EndpointWithSecret {
    #[serde(flatten)]
    endpoint: EndpointView,
    secret: String,
}

impl Outcome for WithSecret {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.endpoint.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.endpoint.committed(stamp);
    }

    fn reply(self) -> Result<Reply> {
        Ok(Reply::data(EndpointWithSecret {
            endpoint: EndpointView::of(&self.endpoint),
            secret: self.secret,
        }))
    }
}

struct EndpointList(Vec<WebhookEndpoint>);

impl Outcome for EndpointList {
    fn reply(self) -> Result<Reply> {
        let views: Vec<EndpointView> = self.0.iter().map(EndpointView::of).collect();
        Ok(Reply::data(views))
    }
}

impl ReadOnly for EndpointList {}

struct Queuing {
    delivery: WebhookDelivery,
    queued: Queued,
}

impl Outcome for Queuing {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.delivery.plan(plan)?;
        self.queued.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.delivery.committed(stamp);
    }

    fn reply(self) -> Result<Reply> {
        Ok(Reply::data(DeliveryView::of(&self.delivery)))
    }
}

#[derive(Serialize)]
struct DeliveryView {
    id: String,
    endpoint: String,
    event: String,
    body: String,
    attempts: i64,
    attempted_at: Option<Instant>,
    status: Option<i64>,
    error: Option<String>,
    next_attempt_at: Option<Instant>,
    delivered_at: Option<Instant>,
    failed_at: Option<Instant>,
    created_at: Option<Instant>,
}

impl DeliveryView {
    fn of(delivery: &WebhookDelivery) -> Self {
        Self {
            id: delivery.id().to_string(),
            endpoint: delivery.endpoint().to_string(),
            event: delivery.event().to_owned(),
            body: delivery.body().to_owned(),
            attempts: delivery.attempts(),
            attempted_at: delivery.attempted_at(),
            status: delivery.status(),
            error: delivery.error().map(str::to_owned),
            next_attempt_at: delivery.next_attempt_at(),
            delivered_at: delivery.delivered_at(),
            failed_at: delivery.failed_at(),
            created_at: delivery.created_at(),
        }
    }
}

struct DeliveryList(Vec<WebhookDelivery>);

impl Outcome for DeliveryList {
    fn reply(self) -> Result<Reply> {
        let views: Vec<DeliveryView> = self.0.iter().map(DeliveryView::of).collect();
        Ok(Reply::data(views))
    }
}

impl ReadOnly for DeliveryList {}

#[derive(Serialize)]
struct EventView {
    name: &'static str,
    summary: &'static str,
}

struct EventList(&'static [WebhookEvent]);

impl Outcome for EventList {
    fn reply(self) -> Result<Reply> {
        let views: Vec<EventView> = self
            .0
            .iter()
            .map(|event| EventView {
                name: event.name,
                summary: event.summary,
            })
            .collect();
        Ok(Reply::data(views))
    }
}

impl ReadOnly for EventList {}
