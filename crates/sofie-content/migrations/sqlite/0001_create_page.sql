CREATE TABLE sofie_content_page (
    id uuid PRIMARY KEY NOT NULL,
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
CREATE UNIQUE INDEX sofie_content_page_slug ON sofie_content_page (slug);
CREATE INDEX sofie_content_page_published ON sofie_content_page (published_at)
    WHERE published_at IS NOT NULL;
