CREATE TABLE sofie_billing_subscription (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    subscriber uuid NOT NULL,
    "plan" text NOT NULL,
    status text NOT NULL,
    seats bigint NOT NULL,
    trial_ends_at timestamptz,
    period_start timestamptz NOT NULL,
    period_ends_at timestamptz,
    cancels_at_period_end boolean NOT NULL,
    provider text,
    customer text,
    reference text,
    provider_changed_at timestamptz,
    created_at timestamptz NOT NULL,
    changed_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX sofie_billing_subscription_subscriber
    ON sofie_billing_subscription (subscriber);

CREATE TABLE sofie_billing_purchase (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    subscriber uuid NOT NULL,
    product text NOT NULL,
    quantity bigint NOT NULL,
    provider text NOT NULL,
    reference text NOT NULL,
    purchased_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX sofie_billing_purchase_reference ON sofie_billing_purchase (reference);
CREATE INDEX sofie_billing_purchase_subscriber ON sofie_billing_purchase (subscriber);

CREATE TABLE sofie_billing_usage (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    subscriber uuid NOT NULL,
    quota text NOT NULL,
    period_start timestamptz NOT NULL,
    used bigint NOT NULL
);
CREATE UNIQUE INDEX sofie_billing_usage_period
    ON sofie_billing_usage (subscriber, quota, period_start);
