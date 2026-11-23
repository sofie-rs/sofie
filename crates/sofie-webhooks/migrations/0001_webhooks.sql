-- Each owner's webhook endpoints and the deliveries of events to them, with their attempts.
CREATE TABLE webhook_endpoint (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    owner_id uuid NOT NULL,
    url text NOT NULL,
    description text,
    events text NOT NULL,
    secret text NOT NULL,
    previous_secret text,
    previous_until timestamptz,
    disabled_at timestamptz,
    disabled_reason text CHECK (disabled_reason IN ('by_owner', 'failing')),
    succeeded_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz
);
CREATE INDEX webhook_endpoint_owner ON webhook_endpoint (owner_id) WHERE deleted_at IS NULL;

CREATE TABLE webhook_delivery (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    owner_id uuid NOT NULL,
    endpoint uuid NOT NULL REFERENCES webhook_endpoint (id),
    event text NOT NULL,
    body text NOT NULL,
    attempts bigint NOT NULL,
    attempted_at timestamptz,
    status bigint,
    error text,
    next_attempt_at timestamptz,
    delivered_at timestamptz,
    failed_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX webhook_delivery_endpoint ON webhook_delivery (endpoint, created_at);
CREATE INDEX webhook_delivery_created ON webhook_delivery (created_at);
