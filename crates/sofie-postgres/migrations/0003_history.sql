-- One row per create, update, delete and restore of a record whose
-- aggregate `RecordsChanges`; written in the commit that changes it.
CREATE TABLE history (
    id uuid PRIMARY KEY,
    record_table text NOT NULL,
    record uuid NOT NULL,
    owner uuid,
    change text NOT NULL,
    fields jsonb NOT NULL,
    actor uuid,
    changed_at timestamptz NOT NULL
);
CREATE INDEX history_record ON history (record_table, record, changed_at);
