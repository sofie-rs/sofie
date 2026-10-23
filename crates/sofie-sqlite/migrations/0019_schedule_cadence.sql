-- The cadence a schedule's next run was computed from; a changed one starts it over.
ALTER TABLE sofie_schedules ADD COLUMN cadence text;
