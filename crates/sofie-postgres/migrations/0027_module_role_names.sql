-- Roles belong to the whole server, so two apps with a module of the same name
-- on one server shared its role and each other's grants. A module's role is
-- now named after the database too; an identifier holds 63 bytes, so a long
-- database name is replaced by its digest.
CREATE FUNCTION module_role_name(module text) RETURNS text
LANGUAGE sql STABLE AS $$
    SELECT CASE
        WHEN octet_length(current_database()) + 1 + octet_length(module) <= 63
            THEN current_database() || '.' || module
        ELSE left(md5(current_database()), 16) || '.' || module
    END
$$;

CREATE OR REPLACE FUNCTION module_role(module text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    role text := sofie.module_role_name(module);
BEGIN
    BEGIN
        EXECUTE format('CREATE ROLE %I NOLOGIN', role);
    EXCEPTION
        WHEN duplicate_object OR unique_violation THEN NULL;
    END;
    BEGIN
        EXECUTE format('GRANT %I TO CURRENT_USER', role);
    EXCEPTION
        WHEN unique_violation THEN NULL;
    END;
    EXECUTE format('ALTER SCHEMA %I OWNER TO %I', module, role);
    EXECUTE format('GRANT ALL ON ALL TABLES IN SCHEMA %I TO %I', module, role);
    EXECUTE format(
        'ALTER DEFAULT PRIVILEGES IN SCHEMA %I GRANT ALL ON TABLES TO %I', module, role
    );
    EXECUTE format('GRANT USAGE ON SCHEMA sofie TO %I', role);
    EXECUTE format('GRANT INSERT ON sofie.messages TO %I', role);
    EXECUTE format('GRANT SELECT, INSERT ON sofie.history TO %I', role);
    EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON sofie.idempotency TO %I', role);
    EXECUTE format('GRANT SELECT ON sofie.flags TO %I', role);
    EXECUTE format('GRANT SELECT, INSERT, UPDATE ON sofie.mirror_forgotten TO %I', role);
    EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON sofie.previous_slug TO %I', role);
END
$$;

-- A role made before is named as its schema and owns it. This database's
-- objects and grants move to the new role; the old one is dropped once no
-- other database still holds anything of it.
DO $$
DECLARE
    module record;
BEGIN
    FOR module IN
        SELECT nspname::text AS name FROM pg_namespace
        WHERE pg_get_userbyid(nspowner) = nspname::text AND nspowner <> CURRENT_USER::regrole
            AND nspname NOT IN ('sofie', 'public')
    LOOP
        PERFORM sofie.module_role(module.name);
        EXECUTE format(
            'REASSIGN OWNED BY %I TO %I', module.name, sofie.module_role_name(module.name)
        );
        EXECUTE format('DROP OWNED BY %I', module.name);
        BEGIN
            EXECUTE format('DROP ROLE %I', module.name);
        EXCEPTION
            WHEN dependent_objects_still_exist THEN NULL;
        END;
    END LOOP;
END
$$;
