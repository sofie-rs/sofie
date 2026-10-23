ALTER TABLE "order" ADD COLUMN placed_at timestamptz NOT NULL DEFAULT now();
ALTER TABLE "order" ALTER COLUMN placed_at DROP DEFAULT;
CREATE INDEX order_pending_placed_at ON "order" (placed_at) WHERE status = 'PENDING';

CREATE TABLE audit_entry (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    "order" uuid NOT NULL,
    action text NOT NULL,
    recorded_at timestamptz NOT NULL
);
CREATE INDEX audit_entry_order ON audit_entry ("order");
