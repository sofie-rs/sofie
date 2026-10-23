-- What a session signed in as, so ending an account's sessions for one organization leaves
-- those for its others; null for sessions recorded before.
ALTER TABLE account_session ADD COLUMN principal uuid;
