-- The area the actor acted in, so their job calls other apps as them; null where none was named.
ALTER TABLE sofie_messages ADD COLUMN actor_area text;
