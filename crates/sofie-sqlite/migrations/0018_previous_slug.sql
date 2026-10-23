CREATE TABLE sofie_previous_slug (
    record_table text NOT NULL,
    slug text NOT NULL,
    record uuid NOT NULL,
    changed_at timestamptz NOT NULL,
    PRIMARY KEY (record_table, slug)
);
CREATE INDEX sofie_previous_slug_record ON sofie_previous_slug (record_table, record);
