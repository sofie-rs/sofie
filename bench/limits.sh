#!/usr/bin/env bash
# Checks the limits against Loco that need no build: app code lines and the
# crates and Rust lines of the shop's dependency tree, counted exactly as
# bench/run.sh counts them; framework lines are measured, not limited. Exits
# non-zero when one is broken. Run by crates/sofie-check/tests/limits.rs.
set -euo pipefail

BENCH=$(cd "$(dirname "$0")" && pwd)
ROOT=$(dirname "$BENCH")
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT
# shellcheck source=/dev/null
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
# shellcheck source=lib/apps.sh
. "$BENCH/lib/apps.sh"
# shellcheck source=lib/measure.sh
. "$BENCH/lib/measure.sh"
# shellcheck source=lib/report.sh
. "$BENCH/lib/report.sh"
# shellcheck source=lib/limits.sh
. "$BENCH/lib/limits.sh"

measure_code sofie
measure_size sofie
check_limits static
