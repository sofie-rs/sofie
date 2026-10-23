-- Orders placed before accounts existed belong to the nil id, which no
-- account ever has (account ids are UUIDv7), so nobody sees them.
ALTER TABLE "order" ADD COLUMN account uuid NOT NULL
    DEFAULT '00000000-0000-0000-0000-000000000000';
ALTER TABLE "order" ALTER COLUMN account DROP DEFAULT;
CREATE INDEX order_account ON "order" (account);
