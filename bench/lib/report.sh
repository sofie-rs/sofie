# Prints the markdown report from out/*.values, sourced by run.sh.

value() { # <app> <key>: the recorded value, or "not run"
    local found
    found=$(awk -F '\t' -v key="$2" '$1 == key { value = $2 } END { print value }' "$OUT/$1.values" 2>/dev/null)
    printf '%s' "${found:-not run}"
}

problems() { # what failed in this run, from the *_error records
    local app lines
    lines=$(for app in sofie loco; do
        awk -F '\t' -v app="$(setting "$app" NAME)" '$1 ~ /_error$/ { printf "- %s: %s\n", app, $2 }' \
            "$OUT/$app.values" 2>/dev/null
    done)
    printf '%s' "${lines:-None.}"
}

row() { # <label> <key>
    printf '| %s | %s | %s |\n' "$1" "$(value sofie "$2")" "$(value loco "$2")"
}

ratio_row() { # <label> <key>: Loco's value over the framework's
    local sofie loco
    sofie=$(value sofie "$2")
    loco=$(value loco "$2")
    printf '| %s | %s | %s | %s |\n' "$1" "$sofie" "$loco" \
        "$(awk -v f="$sofie" -v l="$loco" 'BEGIN { if (f + 0 > 0 && l + 0 > 0) printf "%.2f", l / f; else print "-" }')"
}

table_header() {
    printf '| %s | %s (`examples/shop`) | Loco (`bench/loco-shop`) |\n|---|---:|---:|\n' "$1" "$SOFIE_NAME"
}

ratio_header() {
    printf '| %s | %s | Loco | Loco / %s |\n|---|---:|---:|---:|\n' "$1" "$SOFIE_NAME" "$SOFIE_NAME"
}

file_list() { # <app>
    local listing="$OUT/$1-files.tsv"
    [ -f "$listing" ] || { echo "_not run_"; return; }
    printf '| File | Kind | Group | Lines |\n|---|---|---|---:|\n'
    awk -F '\t' '{ printf "| `%s` | %s | %s | %s |\n", $4, $1, $2, $3 }' "$listing"
}

machine() {
    printf -- '- Date: %s\n' "$(value machine started_at)"
    printf -- '- CPU: %s, %s cores (%s performance + %s efficiency), %s logical\n' \
        "$(sysctl -n machdep.cpu.brand_string)" "$(sysctl -n hw.physicalcpu)" \
        "$(sysctl -n hw.perflevel0.physicalcpu 2>/dev/null || echo '?')" \
        "$(sysctl -n hw.perflevel1.physicalcpu 2>/dev/null || echo 0)" "$(sysctl -n hw.logicalcpu)"
    printf -- '- Memory: %s GB\n' "$(($(sysctl -n hw.memsize) / 1073741824))"
    printf -- '- OS: macOS %s (%s)\n' "$(sw_vers -productVersion)" "$(sw_vers -buildVersion)"
    printf -- '- Toolchain: %s\n' "$(rustc --version)"
    printf -- '- Postgres: %s, on the same machine\n' \
        "$(psql -tA "$PG/postgres" -c 'SHOW server_version' 2>/dev/null || echo unreachable)"
    printf -- '- Load generator: %s, %s threads\n' "$(oha --version)" "$OHA_THREADS"
    printf -- '- Load average when the run started: %s\n' "$(value machine load)"
    printf -- '- Run: `APPS="%s" SECTIONS="%s" SKIP="%s" bench/run.sh%s` (%s mode)\n' \
        "$APPS" "$SECTIONS" "$SKIP" "$([ "$MODE" = full ] && echo ' --full')" "$MODE"
}

report() {
    cat <<EOF
# Framework vs Loco: measurements

Produced by \`bench/run.sh\`; how each number is measured is in
[README.md](README.md). "not run" means that part was not measured in this run.

## Machine

$(machine)

## App code

Non-blank, non-comment lines the developer writes and keeps. Generated files
that are regenerated and never edited are left out (the framework's
\`app/src/wiring.rs\`, Loco's \`src/models/_entities/\`); files a generator wrote
once and the developer then owns are counted.

$(table_header Kind)
$(row Rust code_rust)
$(row Templates code_templates)
$(row "Migrations" code_migrations)
$(row Config code_config)
$(row Other code_other)
$(row "**Total**" code_total)
$(row "of which accounts (sign-up, sign-in, users)" code_auth)
$(row "Files counted" code_files)
$(row "Tests (not in the total)" code_tests)

## Framework and dependencies

$(table_header Measure)
$(row "Framework crates" framework_crates)
$(row "Framework source, Rust lines (all its crates)" framework_lines)
$(row "Framework Rust lines compiled into the app" framework_lines_in_tree)
$(row "Crates in the app's dependency tree (with the app's own)" tree_crates)
$(row "Crates compiled for the app, excluding the app's own" tree_dependency_crates)
$(row "Rust lines in those crates" tree_rust_lines)

## Build

Build times are measured only by \`run.sh --full\`. Release profile for both: opt-level 3, no LTO, 16 codegen units, no debug
info, not incremental (Cargo's defaults, set explicitly).

$(ratio_header Measure)
$(ratio_row "Clean release build (s)" build_clean_s)
$(ratio_row "Release rebuild after touching one handler file (s)" build_incremental_s)
$(ratio_row "Release binary (MB)" binary_mb)

## Runtime

Release binaries, each against its own database on the same Postgres, seeded
with 2 users who signed up through the app and $ORDERS_PER_USER orders each.
\`oha\` warms up for $WARMUP, then runs $DURATION per scenario at $CONCURRENCY connections as the first user;
both apps use a pool of $POOL_SIZE database connections, and their jobs run
2 at a time (both defaults) on a pool of $WORKER_POOL_SIZE of their own (Loco's queue default).
Requests/s counts only answers with the expected status.

$(ratio_header Scenario)
$(ratio_row "JSON order list: requests/s" list_json_rps)
$(ratio_row "JSON order list: p50 (ms)" list_json_p50_ms)
$(ratio_row "JSON order list: p99 (ms)" list_json_p99_ms)
$(ratio_row "JSON order list: errors (%)" list_json_errors_pct)
$(ratio_row "JSON order list: bytes per answer" list_json_bytes)
$(ratio_row "HTML order page: requests/s" page_html_rps)
$(ratio_row "HTML order page: p50 (ms)" page_html_p50_ms)
$(ratio_row "HTML order page: p99 (ms)" page_html_p99_ms)
$(ratio_row "HTML order page: errors (%)" page_html_errors_pct)
$(ratio_row "HTML order page: bytes per answer" page_html_bytes)
$(ratio_row "Place order (form post): requests/s" place_form_rps)
$(ratio_row "Place order (form post): p50 (ms)" place_form_p50_ms)
$(ratio_row "Place order (form post): p99 (ms)" place_form_p99_ms)
$(ratio_row "Place order (form post): errors (%)" place_form_errors_pct)
$(ratio_row "Orders placed" place_form_ok)
$(ratio_row "Audit rows written per second during the order run" audited_per_s)
$(ratio_row "Orders without their audit row when the order run ended" audit_backlog)
$(ratio_row "Audit rows written from the order run's start to $AUDIT_WAIT s after it" audited_10s)
$(ratio_row "RSS after the runs (MB)" rss_mb)
$(ratio_row "Start to first successful request (ms)" startup_ms)
$(ratio_row "Machine load (1-min average) when the app's runs began" runtime_load)

## Limits against Loco

The framework's values against Loco's in \`loco-baseline.json\`, which holds
every ceiling; \`run.sh --full\` exits non-zero when one is broken.

$(check_limits || true)

## Problems in this run

$(problems)

## Files counted

### $SOFIE_NAME

$(file_list sofie)

### Loco

$(file_list loco)

## Where the comparison is uneven

See [README.md](README.md#where-the-comparison-is-uneven).
EOF
}
