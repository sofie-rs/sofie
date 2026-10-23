#!/usr/bin/env bash
# Measures the framework's example shop against its Loco twin and writes
# bench/results.md. What is measured, and how, is in bench/README.md.
#
#   bench/run.sh                                   quick run, both apps (1-2 min)
#   bench/run.sh --full                            thorough run (~12 min)
#   APPS=loco SECTIONS="code runtime" bench/run.sh only part of it
#   SKIP="sofie:runtime" bench/run.sh                  all but one app's section
#   SOFIE_PORT=18080 bench/run.sh                      the framework elsewhere than 8080
#
# Quick mode reuses release binaries already in bench/target (building only a
# missing one), so it reports no build times, and runs smaller data and
# shorter load. Full mode builds both apps from clean and runs the long load,
# and exits non-zero when the framework breaks a limit in loco-baseline.json.
#
# Sections not run, and apps not run, show as "not run" in the report; every
# run rewrites results.md from scratch.
set -euo pipefail

BENCH=$(cd "$(dirname "$0")" && pwd)
ROOT=$(dirname "$BENCH")

# ---- What to run --------------------------------------------------------------
APPS=${APPS:-"sofie loco"}
SECTIONS=${SECTIONS:-"code size build runtime"}
SKIP=${SKIP:-}
MODE=quick
[ "${1:-}" = --full ] && MODE=full
if [ "$MODE" = full ]; then
    DURATION=${DURATION:-30s} WARMUP=${WARMUP:-5s} ORDERS_PER_USER=${ORDERS_PER_USER:-1000}
else
    DURATION=${DURATION:-5s} WARMUP=${WARMUP:-2s} ORDERS_PER_USER=${ORDERS_PER_USER:-200}
fi
CONCURRENCY=${CONCURRENCY:-64}
# How long after the order run the audit rows are counted.
AUDIT_WAIT=${AUDIT_WAIT:-$([ "$MODE" = full ] && echo 10 || echo 3)}
# oha shares the machine with the app and Postgres; a few threads keep it from
# taking the cores the app needs. Same for both apps.
OHA_THREADS=${OHA_THREADS:-4}
PG=${PG:-postgres://sofie:sofie@127.0.0.1:25432}
# The members' password is the one `sofie bench` and `sofie seed` sign in with.
PASSWORD=${PASSWORD:-$(sed -n 's/^pub(crate) const MEMBER_PASSWORD: &str = "\(.*\)";$/\1/p' \
    "$ROOT/crates/sofie-cli/src/local_app.rs")}
[ -n "$PASSWORD" ] || { echo "run.sh: no MEMBER_PASSWORD in crates/sofie-cli/src/local_app.rs" >&2; exit 1; }

# Both release builds use Cargo's default release profile, stated here so it is
# the same for both whatever their manifests say.
export CARGO_PROFILE_RELEASE_OPT_LEVEL=3
export CARGO_PROFILE_RELEASE_DEBUG=false
export CARGO_PROFILE_RELEASE_LTO=false
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
export CARGO_PROFILE_RELEASE_PANIC=unwind
export CARGO_PROFILE_RELEASE_INCREMENTAL=false
export CARGO_PROFILE_RELEASE_STRIP=none

# Both apps get the same connection pool: the framework's default (deadpool:
# logical CPUs x 2), passed to Loco through its documented DB_MAX_CONNECTIONS.
POOL_SIZE=$(($(sysctl -n hw.logicalcpu) * 2))
# Loco's job queue has a pool of its own of 20 connections; the framework's
# worker gets one of that size, so the two apps' jobs run on the same
# resources. Both run 2 jobs at a time by default.
WORKER_POOL_SIZE=20

# shellcheck source=lib/apps.sh
. "$BENCH/lib/apps.sh"

OUT="$BENCH/out"
. "$HOME/.cargo/env"
# shellcheck source=lib/measure.sh
. "$BENCH/lib/measure.sh"
# shellcheck source=lib/report.sh
. "$BENCH/lib/report.sh"
# shellcheck source=lib/limits.sh
. "$BENCH/lib/limits.sh"

main() {
    rm -rf "$OUT"
    mkdir -p "$OUT"
    record machine started_at "$(date -u '+%Y-%m-%d %H:%M UTC')"
    record machine load "$(sysctl -n vm.loadavg | tr -d '{}' | awk '{$1=$1};1')"
    for section in $SECTIONS; do
        for app in $APPS; do
            case " $SKIP " in *" $app:$section "*) continue ;; esac
            log "$section: $app"
            "measure_$section" "$app"
        done
    done
    report | tee "$BENCH/results.md"
    if [ "$MODE" = full ] && ! check_limits >/dev/null; then
        log "a limit against Loco is broken; see \"Limits against Loco\" in results.md"
        exit 1
    fi
}

main
