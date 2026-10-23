-- One subscription per owner: the plan it holds and where it stands with the
-- payment provider, whose changes arrive by webhook.
CREATE TABLE subscription (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    subscriber uuid NOT NULL,
    plan text NOT NULL,
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
CREATE UNIQUE INDEX subscription_subscriber ON subscription (subscriber);

CREATE TABLE purchase (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    subscriber uuid NOT NULL,
    product text NOT NULL,
    quantity bigint NOT NULL,
    provider text NOT NULL,
    reference text NOT NULL,
    purchased_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX purchase_reference ON purchase (reference);
CREATE INDEX purchase_subscriber ON purchase (subscriber);

-- What each owner used of a quota in one period; the next period starts a new row.
CREATE TABLE usage (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    subscriber uuid NOT NULL,
    quota text NOT NULL,
    period_start timestamptz NOT NULL,
    used bigint NOT NULL
);
CREATE UNIQUE INDEX usage_period ON usage (subscriber, quota, period_start);
