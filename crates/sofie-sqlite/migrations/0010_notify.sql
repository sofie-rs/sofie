CREATE TABLE sofie_notify_settings (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    principal uuid NOT NULL UNIQUE,
    zone text,
    quiet_from text,
    quiet_until text,
    digest text NOT NULL
);

CREATE TABLE sofie_notify_choices (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    principal uuid NOT NULL,
    topic text NOT NULL,
    channel text NOT NULL,
    enabled boolean NOT NULL,
    UNIQUE (principal, topic, channel)
);

CREATE TABLE sofie_inbox (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    principal uuid NOT NULL,
    topic text NOT NULL,
    title text NOT NULL,
    body text NOT NULL,
    link text,
    created_at timestamptz NOT NULL,
    read_at timestamptz
);
CREATE INDEX sofie_inbox_principal ON sofie_inbox (principal, created_at);

CREATE TABLE sofie_notify_digest_items (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    principal uuid NOT NULL,
    email text NOT NULL,
    title text NOT NULL,
    body text NOT NULL,
    link text,
    created_at timestamptz NOT NULL,
    sent_at timestamptz
);
CREATE INDEX sofie_notify_digest_items_pending ON sofie_notify_digest_items (principal)
    WHERE sent_at IS NULL;

CREATE TABLE sofie_push_subscriptions (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    principal uuid NOT NULL,
    endpoint text NOT NULL UNIQUE,
    p256dh text NOT NULL,
    auth text NOT NULL,
    created_at timestamptz NOT NULL,
    gone_at timestamptz
);
CREATE INDEX sofie_push_subscriptions_principal ON sofie_push_subscriptions (principal);
