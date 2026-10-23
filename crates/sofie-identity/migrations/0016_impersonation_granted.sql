-- What the member held when its impersonation started, so one it gains since ends it.
ALTER TABLE impersonation ADD COLUMN granted text NOT NULL DEFAULT '';
