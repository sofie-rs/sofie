# Prints the name of every file module (`mod name;`) a Rust file declares
# that the build compiles, one per line.
#
#   awk -v features=",engine,otlp," -f modules.awk FILE
#
# A module under `#[cfg(...)]` is kept when the predicate holds for the
# crate's enabled features: `feature = "x"`, `not`, `any` and `all` are
# evaluated, `test` is false, and any other predicate (a target, a flag) is
# taken as true, so what cannot be decided is counted rather than dropped.

function predicate_value(predicate,    name) {
    if (predicate == "test") return 0
    if (predicate ~ /^feature *= *"[^"]*"$/) {
        name = predicate
        sub(/^feature *= *"/, "", name)
        sub(/"$/, "", name)
        return index(features, "," name ",") > 0
    }
    return 1
}

# Reduces a cfg predicate to 1 or 0 by evaluating its innermost
# `not(...)`, `any(...)` and `all(...)` until none is left.
function holds(predicate,    inner, operator, arguments, count, index_, result, part) {
    gsub(/[ \t]+/, " ", predicate)
    while (match(predicate, /(not|any|all)\([^()]*\)/)) {
        inner = substr(predicate, RSTART, RLENGTH)
        operator = substr(inner, 1, 3)
        arguments = substr(inner, 5, length(inner) - 5)
        count = split(arguments, parts, ",")
        result = (operator == "all")
        if (operator == "not") result = !value_of(parts[1])
        for (index_ = 1; index_ <= count && operator != "not"; index_++) {
            part = value_of(parts[index_])
            if (operator == "any" && part) result = 1
            if (operator == "all" && !part) result = 0
        }
        predicate = substr(predicate, 1, RSTART - 1) result substr(predicate, RSTART + RLENGTH)
    }
    return value_of(predicate)
}

function value_of(text) {
    sub(/^ +/, "", text)
    sub(/ +$/, "", text)
    if (text == "0" || text == "1") return text + 0
    return predicate_value(text)
}

{
    line = $0
    sub(/^[ \t]+/, "", line)
}

# A `#[cfg(...)]` rustfmt wrapped over several lines is joined first.
pending != "" {
    pending = pending " " line
    if (line !~ /\)\]$/) next
    line = pending
    pending = ""
}

line ~ /^#\[cfg\([^\]]*$/ {
    pending = line
    next
}

line ~ /^#\[cfg\(.*\)\]$/ {
    predicate = line
    sub(/^#\[cfg\(/, "", predicate)
    sub(/\)\]$/, "", predicate)
    enabled = enabled && holds(predicate)
    next
}

line ~ /^#\[/ { next }

line ~ /^(pub(\([a-z]+\))? )?mod [A-Za-z_0-9]+ *;/ {
    if (enabled) {
        name = line
        sub(/^(pub(\([a-z]+\))? )?mod /, "", name)
        sub(/ *;.*$/, "", name)
        print name
    }
}

{ enabled = 1 }

BEGIN { enabled = 1 }
