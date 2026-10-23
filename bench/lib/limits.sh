# The framework's limits against Loco, sourced by run.sh and limits.sh.
# Loco's numbers and every ceiling live in loco-baseline.json and nowhere
# else; a ceiling is only ever tightened.

BASELINE="$BENCH/loco-baseline.json"

# Prints "<key>\t<label>\t<at_most|at_least>\t<bound>\t<Loco's value or null>"
# per limit; with "static", only those measured without building.
limit_rows() {
    jq -r --arg only "${1:-}" '.loco as $loco | .limits[]
        | select($only != "static" or .static)
        | [.key, .label, (if .at_most != null then "at_most" else "at_least" end),
           (.at_most // .at_least), ($loco[.key] // "null")] | @tsv' "$BASELINE"
}

# Prints a markdown table of the framework's values against the limits and
# returns non-zero when one is broken; a value not measured, or without a
# Loco baseline, is shown but breaks nothing.
check_limits() { # [static]
    local key label kind bound loco ours broken=0 verdict
    printf '| Measure | %s | Loco baseline | Ratio | Limit | Verdict |\n|---|---:|---:|---:|---|---|\n' "$SOFIE_NAME"
    while IFS=$'\t' read -r key label kind bound loco; do
        ours=$(value sofie "$key")
        verdict=$(awk -v ours="$ours" -v loco="$loco" -v kind="$kind" -v bound="$bound" 'BEGIN {
            if (loco == "null") { print "no baseline\t-"; exit }
            if (ours == "not run") { print "not run\t-"; exit }
            ratio = ours / loco
            ok = (kind == "at_most") ? ratio <= bound : ratio >= bound
            printf "%s\t%.3f\n", (ok ? "ok" : "**BROKEN**"), ratio }')
        [ "${verdict%%$'\t'*}" = "**BROKEN**" ] && broken=$((broken + 1))
        printf '| %s | %s | %s | %s | %s %s × Loco | %s |\n' "$label" "$ours" "$loco" \
            "${verdict#*$'\t'}" "$([ "$kind" = at_most ] && echo '≤' || echo '≥')" "$bound" \
            "${verdict%%$'\t'*}"
    done < <(limit_rows "${1:-}")
    return "$broken"
}
