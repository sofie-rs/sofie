-- A duplicate grant is refused under its field's name, as resources name
-- a unique index `<table>_<field>`.
ALTER INDEX role_grant_held RENAME TO role_grant_permission;
