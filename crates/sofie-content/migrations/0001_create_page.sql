CREATE TABLE page (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    title text NOT NULL,
    slug text NOT NULL,
    summary text NOT NULL,
    body text NOT NULL,
    image text,
    published_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX page_slug ON page (slug);
CREATE INDEX page_published ON page (published_at) WHERE published_at IS NOT NULL;
