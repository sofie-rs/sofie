-- sofie-notify: each principal's notification settings and channel choices,
-- its inbox, mails held for its digest and its browsers' push subscriptions.
CREATE TABLE notify_settings (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    principal uuid NOT NULL UNIQUE,
    zone text,
    quiet_from text,
    quiet_until text,
    digest text NOT NULL
);

CREATE TABLE notify_choices (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    principal uuid NOT NULL,
    topic text NOT NULL,
    channel text NOT NULL,
    enabled boolean NOT NULL,
    UNIQUE (principal, topic, channel)
);

CREATE TABLE inbox (
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
CREATE INDEX inbox_principal ON inbox (principal, created_at);

CREATE TABLE notify_digest_items (
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
CREATE INDEX notify_digest_items_pending ON notify_digest_items (principal) WHERE sent_at IS NULL;

CREATE TABLE push_subscriptions (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    principal uuid NOT NULL,
    endpoint text NOT NULL UNIQUE,
    p256dh text NOT NULL,
    auth text NOT NULL,
    created_at timestamptz NOT NULL,
    gone_at timestamptz
);
CREATE INDEX push_subscriptions_principal ON push_subscriptions (principal);
