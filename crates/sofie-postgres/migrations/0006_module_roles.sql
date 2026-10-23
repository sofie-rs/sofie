-- A module's database role, which each module's `0000_module_role.sql`
-- (written by `sofie gen`) makes through this function, so the grants live in
-- the framework and a module's migration holds only its name. The role
-- owns the module's schema and reaches no other module's; of the
-- framework's tables it gets what a commit writes. The app's login
-- migrates and becomes a member, so it reads and writes every schema itself
-- and runs a module's raw SQL as the module's role.
CREATE FUNCTION module_role(module text) RETURNS void
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
END
$$;
