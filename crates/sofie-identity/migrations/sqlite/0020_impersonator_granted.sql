ALTER TABLE sofie_identity_impersonation ADD COLUMN impersonator_area text NOT NULL DEFAULT '';
ALTER TABLE sofie_identity_impersonation ADD COLUMN impersonator_granted text NOT NULL DEFAULT '';
