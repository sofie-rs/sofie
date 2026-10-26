-- Tells two live provider subscriptions of one subscriber apart: the later started is followed.
ALTER TABLE subscription ADD COLUMN provider_started_at timestamptz;
