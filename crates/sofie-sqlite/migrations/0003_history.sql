CREATE TABLE sofie_history (
    id uuid PRIMARY KEY,
    record_table text NOT NULL,
    record uuid NOT NULL,
    owner uuid,
    change text NOT NULL,
    fields jsonb NOT NULL,
    actor uuid,
    changed_at timestamptz NOT NULL
);
CREATE INDEX sofie_history_record ON sofie_history (record_table, record, changed_at);
