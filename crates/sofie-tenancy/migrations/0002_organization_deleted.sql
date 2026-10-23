-- An organization an owner deleted stays as the record its members' histories name.
ALTER TABLE organization ADD COLUMN deleted_at timestamptz;
