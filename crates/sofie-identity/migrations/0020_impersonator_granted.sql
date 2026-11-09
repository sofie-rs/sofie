-- What the impersonator held where it started, so one it loses since ends the impersonation.
ALTER TABLE impersonation
    ADD COLUMN impersonator_area text NOT NULL DEFAULT '',
    ADD COLUMN impersonator_granted text NOT NULL DEFAULT '';
