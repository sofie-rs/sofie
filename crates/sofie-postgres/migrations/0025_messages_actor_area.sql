-- The area the actor acted in, so their job calls other apps as them; null where none was named.
ALTER TABLE messages ADD COLUMN actor_area text;
