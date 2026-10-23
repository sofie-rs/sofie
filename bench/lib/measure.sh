# Measurements, sourced by run.sh. Each `measure_<section> <app>` records its
# numbers with `record <app> <key> <value>` into out/<app>.values; report.sh
# reads them back. <app> is `sofie` or `loco`; `setting <app> NAME` reads the
# app's SOFIE_NAME / LOCO_NAME variables from run.sh.

log() {
    printf '>> %s\n' "$*" >&2
}

record() {
    printf '%s\t%s\n' "$2" "$3" >>"$OUT/$1.values"
}

setting() {
    local name
    name="$(printf '%s' "$1" | tr '[:lower:]' '[:upper:]')_$2"
    printf '%s' "${!name}"
}

now() {
    perl -MTime::HiRes=time -e 'printf "%.3f\n", time'
}

seconds_since() {
    awk -v start="$1" -v end="$(now)" 'BEGIN { printf "%.1f", end - start }'
}

count_lines() { # <syntax> [skip_tests] <file>...  -> "<count>\t<file>" per file
    local syntax=$1 skip=$2
    shift 2
    [ $# -gt 0 ] || return 0
    awk -v kind="$syntax" -v skip_tests="$skip" -f "$BENCH/lib/count.awk" "$@"
}

sum_column() {
    awk -F '\t' '{ total += $1 } END { print total + 0 }'
}

# ---- App code -----------------------------------------------------------------

# Prints "<category>\t<group>\t<relative path>" for every file the developer
# keeps in the app's tree, by the rule in README.md.
app_files() {
    local app=$1 dir
    dir=$(setting "$app" DIR)
    (cd "$dir" && find . -type f \
        ! -path './target/*' ! -path './.github/*' ! -path './.claude/*' \
        ! -path './src/models/_entities/*' ! -path './app/src/wiring.rs' ! -path './modules/*/src/wiring.rs' \
        ! -path '*/gen/*' ! -path './bench/*' ! -name 'Dockerfile' ! -name '*.dockerignore' \
        ! -name '*.md' ! -name 'Cargo.lock' \
        ! -name '.gitignore' ! -name '.env' ! -name '.mcp.json' ! -name '.DS_Store' ! -name '*.png' | sed 's|^\./||' | sort) |
        while read -r path; do
            printf '%s\t%s\t%s\n' "$(category "$path")" "$(group "$app" "$path")" "$path"
        done
}

category() {
    case $1 in
    tests/* | */tests/* | src/fixtures/* | *.snap) echo tests ;;
    *.toml | *.yaml | *.yml | .env* | */.env*) echo config ;;
    migration/* | */migrations/* | migrations/*) echo migrations ;;
    *.rs) echo rust ;;
    *.html | *.t | *.svelte | *.ftl) echo templates ;;
    *) echo other ;;
    esac
}

# Loco's starter generates the whole account system into the app; the
# framework ships it as the sofie-identity crate. "auth" marks the Loco files
# that exist only for accounts, so the report can show the app without them.
group() {
    case "$1:$2" in
    loco:src/controllers/auth.rs | loco:src/models/users.rs | loco:src/views/auth.rs | \
        loco:src/mailers/* | loco:src/tasks/user_*.rs | loco:migration/src/m20220101_000001_users.rs | \
        loco:src/controllers/sessions.rs | loco:assets/views/sessions/*)
        echo auth
        ;;
    *) echo shop ;;
    esac
}

syntax_of() {
    case $1 in
    *.rs) echo rust ;;
    *.sql) echo sql ;;
    *.html | *.t | *.svelte) echo markup ;;
    *) echo hash ;;
    esac
}

measure_code() {
    local app=$1 dir listing category group path lines
    dir=$(setting "$app" DIR)
    listing="$OUT/$app-files.tsv"
    : >"$listing"
    app_files "$app" | while IFS=$'\t' read -r category group path; do
        lines=$(count_lines "$(syntax_of "$path")" 0 "$dir/$path" | cut -f1)
        printf '%s\t%s\t%s\t%s\n' "$category" "$group" "${lines:-0}" "$path" >>"$listing"
    done
    for category in rust templates migrations config other tests; do
        record "$app" "code_$category" "$(awk -F '\t' -v c="$category" '$1 == c { t += $3 } END { print t + 0 }' "$listing")"
    done
    record "$app" code_total "$(awk -F '\t' '$1 != "tests" { t += $3 } END { print t + 0 }' "$listing")"
    record "$app" code_auth "$(awk -F '\t' '$1 != "tests" && $2 == "auth" { t += $3 } END { print t + 0 }' "$listing")"
    record "$app" code_files "$(awk -F '\t' '$1 != "tests"' "$listing" | wc -l | tr -d ' ')"
}

# ---- Framework size and dependency tree ---------------------------------------

registry_dir() { # <crate> <version>
    local found
    for found in "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/"$1-$2"; do
        [ -d "$found" ] && { printf '%s' "$found"; return; }
    done
}

rust_lines_in() { # <crate directory>: library code, tests left out
    find "$1" -name '*.rs' ! -path "$1/tests/*" ! -path "$1/benches/*" \
        ! -path "$1/examples/*" ! -path '*/target/*' ! -name 'tests.rs' -print0 |
        xargs -0 awk -v kind=rust -v skip_tests=1 -f "$BENCH/lib/count.awk" | sum_column
}

# The source files a build of the crate in <directory> compiles with
# <features> enabled (",a,b,"): its root and every file module reached from
# it whose `#[cfg]` holds, tests left out.
compiled_files() { # <crate directory> <features>
    local dir=$1 features=$2 root file module_dir name
    root="$dir/src/lib.rs"
    [ -f "$root" ] || root="$dir/src/main.rs"
    local queue=("$root")
    while [ ${#queue[@]} -gt 0 ]; do
        file=${queue[0]}
        queue=("${queue[@]:1}")
        printf '%s\n' "$file"
        case ${file##*/} in
        lib.rs | main.rs | mod.rs) module_dir=${file%/*} ;;
        *) module_dir=${file%.rs} ;;
        esac
        while read -r name; do
            if [ -f "$module_dir/$name.rs" ]; then
                queue+=("$module_dir/$name.rs")
            elif [ -f "$module_dir/$name/mod.rs" ]; then
                queue+=("$module_dir/$name/mod.rs")
            else
                log "no file for module $name declared in $file"
            fi
        done < <(awk -v features="$features" -f "$BENCH/lib/modules.awk" "$file")
    done
}

compiled_lines() { # <crate directory> <features>
    compiled_files "$1" "$2" | tr '\n' '\0' |
        xargs -0 awk -v kind=rust -v skip_tests=1 -f "$BENCH/lib/count.awk" | sum_column
}

# The crates the app's binary is built from, with the features the build
# enables: "<name>\t<version>\t,<features>,\t<path or empty>"; the
# features come before the path, which `read` would lose when empty.
dependency_tree() {
    local app=$1 manifest package features=()
    if [ "$app" = sofie ]; then
        manifest="$ROOT/Cargo.toml" package=shop features=(--no-default-features)
    else
        manifest="$LOCO_DIR/Cargo.toml" package=loco_shop
    fi
    cargo tree --offline --locked --manifest-path "$manifest" -p "$package" -e normal \
        ${features[@]+"${features[@]}"} --prefix none -f '{p}|{f}' | sed 's/ (\*)$//' | sort -u |
        awk -F '|' '{ split($1, package, " "); path = "";
               if (match($1, /\(\/[^)]*\)/)) path = substr($1, RSTART + 1, RLENGTH - 2);
               sub(/^v/, "", package[2]); printf "%s\t%s\t,%s,\t%s\n", package[1], package[2], $2, path }' |
        awk -F '\t' '!seen[$1 FS $2]++'
}

measure_size() {
    local app=$1
    local tree="$OUT/$app-tree.tsv" app_dir name version features path dir lines total=0 crates=0 own
    app_dir=$(setting "$app" DIR)
    if ! dependency_tree "$app" >"$tree"; then
        record "$app" size_error "cargo tree failed"
        return
    fi
    : >"$OUT/$app-tree-lines.tsv"
    while IFS=$'\t' read -r name version features path; do
        case $path in "$app_dir"*) continue ;; esac
        dir=${path:-$(registry_dir "$name" "$version")}
        if [ -n "$path" ]; then
            # The framework's own crates count only what the app's features
            # compile; registry crates (Loco among them) count whole, as before.
            lines=$(compiled_lines "$dir" "$features")
        elif [ -z "$dir" ]; then
            log "no source for $name $version; counted as 0 lines"
            lines=0
        else
            lines=$(rust_lines_in "$dir")
        fi
        printf '%s\t%s\t%s\n' "$lines" "$name" "$version" >>"$OUT/$app-tree-lines.tsv"
        total=$((total + lines))
        crates=$((crates + 1))
    done <"$tree"
    record "$app" tree_crates "$(wc -l <"$tree" | tr -d ' ')"
    record "$app" tree_dependency_crates "$crates"
    record "$app" tree_rust_lines "$total"
    if [ "$app" = sofie ]; then
        own=0
        for dir in "$ROOT"/crates/*/; do
            own=$((own + $(rust_lines_in "${dir%/}")))
        done
        record sofie framework_lines "$own"
        record sofie framework_crates "$(ls -d "$ROOT"/crates/*/ | wc -l | tr -d ' ')"
        record sofie framework_lines_in_tree "$(awk -F '\t' '$2 ~ /^sofie(-|$)/ { t += $1 } END { print t + 0 }' "$OUT/sofie-tree-lines.tsv")"
    else
        version=$(awk -F '\t' '$1 == "loco-rs" { print $2 }' "$tree")
        record loco framework_lines "$(rust_lines_in "$(registry_dir loco-rs "$version")")"
        record loco framework_crates 1
        record loco framework_version "loco-rs $version"
        record loco framework_lines_in_tree "$(awk -F '\t' '$2 ~ /^loco-/ { t += $1 } END { print t + 0 }' "$OUT/loco-tree-lines.tsv")"
    fi
}

# ---- Build --------------------------------------------------------------------

build() { # <app>: release build of the app's binary into bench/target/<app>
    local app=$1
    if [ "$app" = sofie ]; then
        CARGO_TARGET_DIR="$BENCH/target/sofie" cargo build --release --offline --locked --quiet \
            --manifest-path "$ROOT/Cargo.toml" -p shop --no-default-features --bin "$SOFIE_BINARY"
    else
        CARGO_TARGET_DIR="$BENCH/target/loco" cargo build --release --offline --locked --quiet \
            --manifest-path "$LOCO_DIR/Cargo.toml" --bin "$LOCO_BINARY"
    fi
}

binary() {
    printf '%s' "$BENCH/target/$1/release/$(setting "$1" BINARY)"
}

# Quick mode: the existing binary, or a build when there is none; no timings.
ensure_binary() {
    local app=$1
    [ -x "$(binary "$app")" ] && return 0
    build "$app" >>"$OUT/$app-build.log" 2>&1
}

measure_build() {
    local app=$1 start
    if [ "$MODE" = quick ]; then
        if ! ensure_binary "$app"; then
            record "$app" build_error "release build failed, see out/$app-build.log"
            return
        fi
        record "$app" binary_mb "$(stat -f %z "$(binary "$app")" | awk '{ printf "%.1f", $1 / 1048576 }')"
        return
    fi
    rm -rf "$BENCH/target/$app"
    start=$(now)
    if ! build "$app" >"$OUT/$app-build.log" 2>&1; then
        record "$app" build_error "release build failed, see out/$app-build.log"
        return
    fi
    record "$app" build_clean_s "$(seconds_since "$start")"
    record "$app" binary_mb "$(stat -f %z "$(binary "$app")" | awk '{ printf "%.1f", $1 / 1048576 }')"
    touch "$(setting "$app" HANDLER)"
    start=$(now)
    build "$app" >>"$OUT/$app-build.log" 2>&1
    record "$app" build_incremental_s "$(seconds_since "$start")"
}

# ---- Runtime ------------------------------------------------------------------

database_url() {
    printf '%s/bench_%s' "$PG" "$1"
}

recreate_database() {
    psql -q "$PG/postgres" -c "DROP DATABASE IF EXISTS bench_$1 WITH (FORCE)" \
        -c "CREATE DATABASE bench_$1" >/dev/null
}

start_app() { # <app>: starts it in the background; sets APP_PID
    local app=$1 url
    url=$(database_url "$app")
    if lsof -nP -iTCP:"$(setting "$app" PORT)" -sTCP:LISTEN >/dev/null; then
        log "port $(setting "$app" PORT) is taken by another process"
        return 1
    fi
    if [ "$app" = sofie ]; then
        DATABASE_URL="$url" SOFIE_ADDRESS="127.0.0.1:$SOFIE_PORT" SOFIE_SIGNING_KEYS="$SESSION_KEY" SOFIE_MIGRATE=on \
            SOFIE_WORKER_POOL_SIZE="$WORKER_POOL_SIZE" \
            "$(binary sofie)" >>"$OUT/sofie-server.log" 2>&1 &
    else
        (cd "$LOCO_DIR" && exec env LOCO_ENV=production DATABASE_URL="$url" QUEUE_URL="$url" \
            JWT_SECRET="$JWT_SECRET" HOST="http://127.0.0.1:$LOCO_PORT" \
            MAILER_HOST=localhost MAILER_USER=bench MAILER_PASSWORD=bench \
            BINDING=127.0.0.1 PORT="$LOCO_PORT" DB_MAX_CONNECTIONS="$POOL_SIZE" \
            "$(binary loco)" start --all) >>"$OUT/loco-server.log" 2>&1 &
    fi
    APP_PID=$!
}

stop_app() {
    kill -TERM "$APP_PID" 2>/dev/null || return 0
    for _ in $(seq 1 100); do
        kill -0 "$APP_PID" 2>/dev/null || break
        sleep 0.1
    done
    kill -KILL "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
}

base_url() {
    printf 'http://127.0.0.1:%s' "$(setting "$1" PORT)"
}

wait_ready() { # <app>: polls the public sign-in page until it answers 200
    local url
    url="$(base_url "$1")$(setting "$1" READY)"
    for _ in $(seq 1 6000); do
        [ "$(curl -s -o /dev/null -w '%{http_code}' "$url")" = 200 ] && return 0
        kill -0 "$APP_PID" 2>/dev/null || { log "$1 exited during start"; return 1; }
        sleep 0.005
    done
    return 1
}

# Signs up and in as <email>; prints the session cookie as "name=value".
session_cookie() {
    local app=$1 email=$2 base cookie_name body type
    base=$(base_url "$app")
    cookie_name=$(setting "$app" SESSION_COOKIE)
    if [ "$app" = sofie ]; then
        type=application/json
        body="{\"email\":\"$email\",\"password\":\"$PASSWORD\"}"
    else
        type=application/x-www-form-urlencoded
        body="name=Bench&email=$email&password=$PASSWORD"
    fi
    curl -s -o /dev/null -X POST -H "content-type: $type" -H "origin: $base" \
        --data "$body" "$base$(setting "$app" SIGN_UP)"
    curl -s -o /dev/null -D - -X POST -H "content-type: $type" -H "origin: $base" \
        --data "$body" "$base$(setting "$app" SIGN_IN)" |
        tr -d '\r' | grep -i "^set-cookie: $cookie_name=" | head -1 |
        sed -E 's/^[Ss]et-[Cc]ookie: ([^;]*).*/\1/'
}

seed_orders() {
    printf '%s\nVACUUM ANALYZE;\n' "$(setting "$1" SEED_SQL)" |
        psql -q -v ON_ERROR_STOP=1 -v orders="$ORDERS_PER_USER" "$(database_url "$1")" >/dev/null
}

load() { # <app> <scenario> <expected status> <path> <oha arguments>...
    local app=$1 scenario=$2 expected=$3 path=$4 result
    shift 4
    result="$OUT/$app-$scenario.json"
    oha -z "$DURATION" -c "$CONCURRENCY" --no-tui --output-format json \
        --worker-threads "$OHA_THREADS" -H "cookie: $COOKIE" "$@" \
        "$(base_url "$app")$path" >"$result"
    jq -r --arg expected "$expected" '
        ([.statusCodeDistribution[]] | add // 0) as $answered
        | ([.errorDistribution | to_entries[] | select(.key != "aborted due to deadline") | .value] | add // 0) as $failed
        | (.statusCodeDistribution[$expected] // 0) as $ok
        | ($answered + $failed) as $sent
        | [ $ok, ($ok / .summary.total),
            (.latencyPercentiles.p50 * 1000), (.latencyPercentiles.p99 * 1000),
            (if $sent == 0 then 100 else ($sent - $ok) * 100 / $sent end),
            (.summary.totalData / ([$answered, 1] | max)) ]
        | @tsv' "$result" |
        while IFS=$'\t' read -r ok rps p50 p99 errors bytes; do
            record "$app" "${scenario}_ok" "$ok"
            record "$app" "${scenario}_rps" "$(printf '%.0f' "$rps")"
            record "$app" "${scenario}_p50_ms" "$(printf '%.2f' "$p50")"
            record "$app" "${scenario}_p99_ms" "$(printf '%.2f' "$p99")"
            record "$app" "${scenario}_errors_pct" "$(printf '%.2f' "$errors")"
            record "$app" "${scenario}_bytes" "$(printf '%.0f' "$bytes")"
        done
}

audit_rows() {
    local sql='SELECT count(*) FROM audit_entries'
    [ "$1" = sofie ] && sql='SELECT count(*) FROM "order".audit_entry'
    psql -tA "$(database_url "$1")" -c "$sql"
}

# How fast the audit job kept up while orders came in, over the order run's
# own length as oha timed it, and how many orders it had not reached when the
# run ended.
record_audit_pace() { # <app> <audit rows written during the order run>
    local app=$1 written=$2 seconds placed
    seconds=$(jq -r '.summary.total' "$OUT/$app-place_form.json")
    placed=$(value "$app" place_form_ok)
    record "$app" audited_per_s "$(awk -v w="$written" -v s="$seconds" 'BEGIN { printf "%.0f", w / s }')"
    record "$app" audit_backlog "$((placed - written))"
}

measure_runtime() {
    local app=$1 base start placed_before
    local prepare=build
    [ "$MODE" = quick ] && prepare=ensure_binary
    if ! "$prepare" "$app" >>"$OUT/$app-build.log" 2>&1; then
        record "$app" runtime_error "release build failed, see out/$app-build.log"
        return
    fi
    record "$app" runtime_load "$(sysctl -n vm.loadavg | awk '{ print $2 }')"
    # Fresh signing keys per run, for the framework's session cookie and Loco's JWT.
    SESSION_KEY=$(openssl rand -base64 32)
    JWT_SECRET=$(openssl rand -hex 32)
    base=$(base_url "$app")
    recreate_database "$app"

    # First boot migrates; the users sign up through the app itself.
    if ! { start_app "$app" && wait_ready "$app"; }; then
        record "$app" runtime_error "the app did not start, see out/$app-server.log"
        stop_app
        return
    fi
    COOKIE=$(session_cookie "$app" bench1@example.com)
    session_cookie "$app" bench2@example.com >/dev/null
    if [ -z "$COOKIE" ]; then
        record "$app" runtime_error "signing in failed, see out/$app-server.log"
        stop_app
        return
    fi
    stop_app
    seed_orders "$app"

    # Second boot, on a migrated and seeded database, is the one timed.
    start=$(now)
    start_app "$app" && wait_ready "$app"
    record "$app" startup_ms "$(awk -v s="$start" -v e="$(now)" 'BEGIN { printf "%.0f", (e - s) * 1000 }')"

    local json=(-H 'accept: application/json')
    local html=(-H 'accept: text/html')
    local form=(-m POST -T application/x-www-form-urlencoded -H "origin: $base"
        -H 'sec-fetch-site: same-origin' -d "$(setting "$app" PLACE_FORM)")
    DURATION=$WARMUP load "$app" warmup 200 "$(setting "$app" LIST_JSON)" "${json[@]}"
    load "$app" list_json 200 "$(setting "$app" LIST_JSON)" "${json[@]}"
    load "$app" page_html 200 "$(setting "$app" PAGE)" "${html[@]}"
    placed_before=$(audit_rows "$app")
    load "$app" place_form 303 "$(setting "$app" PLACE)" "${html[@]}" "${form[@]}"
    record_audit_pace "$app" "$(($(audit_rows "$app") - placed_before))"
    record "$app" rss_mb "$(ps -o rss= -p "$APP_PID" | awk '{ printf "%.0f", $1 / 1024 }')"
    sleep "$AUDIT_WAIT"
    record "$app" audited_10s "$(($(audit_rows "$app") - placed_before))"
    stop_app
}
