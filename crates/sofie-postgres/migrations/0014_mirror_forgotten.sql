-- The records a mirror without `deleted_at` dropped, at the version that
-- deleted them, so a change event arriving late cannot bring one back.
CREATE TABLE mirror_forgotten (
    mirror text NOT NULL,
    id uuid NOT NULL,
    version bigint NOT NULL,
    PRIMARY KEY (mirror, id)
);

-- Module roles write it in their commits, those made before as the ones to come.
CREATE OR REPLACE FUNCTION module_role(module text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    BEGIN
        EXECUTE format('CREATE ROLE %I NOLOGIN', module);
    EXCEPTION
        -- Roles belong to the whole server: another database may have made it.
        WHEN duplicate_object OR unique_violation THEN NULL;
    END;
    BEGIN
        EXECUTE format('GRANT %I TO CURRENT_USER', module);
    EXCEPTION
        WHEN unique_violation THEN NULL;
    END;
    EXECUTE format('ALTER SCHEMA %I OWNER TO %I', module, module);
    EXECUTE format('GRANT ALL ON ALL TABLES IN SCHEMA %I TO %I', module, module);
    EXECUTE format(
        'ALTER DEFAULT PRIVILEGES IN SCHEMA %I GRANT ALL ON TABLES TO %I', module, module
    );
    EXECUTE format('GRANT USAGE ON SCHEMA sofie TO %I', module);
    EXECUTE format('GRANT INSERT ON sofie.messages TO %I', module);
    EXECUTE format('GRANT SELECT, INSERT ON sofie.history TO %I', module);
    EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON sofie.idempotency TO %I', module);
    EXECUTE format('GRANT SELECT ON sofie.flags TO %I', module);
    EXECUTE format('GRANT SELECT, INSERT, UPDATE ON sofie.mirror_forgotten TO %I', module);
END
$$;

DO $$
DECLARE
    module record;
BEGIN
    FOR module IN
        SELECT grantee FROM information_schema.role_table_grants
        WHERE table_schema = 'sofie' AND table_name = 'history' AND privilege_type = 'INSERT'
            AND grantee <> CURRENT_USER
    LOOP
        EXECUTE format('GRANT SELECT, INSERT, UPDATE ON sofie.mirror_forgotten TO %I', module.grantee);
    END LOOP;
END
$$;
