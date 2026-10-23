-- A removed passkey's credential may be added again. Built before the old index is dropped,
-- in one transaction, so the credential is never unguarded.
-- sofie:allow migration-lock: one row per passkey, so the build blocks its writes for moments
CREATE UNIQUE INDEX passkey_live_credential ON passkey (credential) WHERE deleted_at IS NULL;
DROP INDEX passkey_credential;
ALTER INDEX passkey_live_credential RENAME TO passkey_credential;
