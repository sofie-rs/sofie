# Counts non-blank, non-comment lines per file and prints "<count>\t<file>".
#
#   awk -v kind=rust|sql|markup|hash [-v skip_tests=1] -f count.awk FILE...
#
# rust:   `//` line comments (doc comments included) and `/* */` blocks.
#         With skip_tests=1, a top-level `#[cfg(test)]` followed by an inline
#         `mod name {` ends the file's count: test modules sit at the bottom.
# sql:    `--` line comments and `/* */` blocks.
# markup: `<!-- -->` and Tera `{# #}` comments (HTML, Tera, mailer templates).
# hash:   `#` line comments (TOML, YAML, Fluent, .env).
# A line holding code and a trailing comment counts as code.

function flush() {
    if (file != "") printf "%d\t%s\n", count, file
}

function trimmed(text) {
    sub(/^[ \t]+/, "", text)
    sub(/[ \t\r]+$/, "", text)
    return text
}

# Removes block comments that start the line; sets block_end while one is
# still open at the end of the line.
function without_leading_comments(text,    rest, position) {
    while (1) {
        if (block_end != "") {
            position = index(text, block_end)
            if (position == 0) return ""
            text = trimmed(substr(text, position + length(block_end)))
            block_end = ""
        }
        if (kind == "markup" && substr(text, 1, 4) == "<!--") {
            block_end = "-->"; text = substr(text, 5)
        } else if (kind == "markup" && substr(text, 1, 2) == "{#") {
            block_end = "#}"; text = substr(text, 3)
        } else if (kind != "markup" && substr(text, 1, 2) == "/*") {
            block_end = "*/"; text = substr(text, 3)
        } else {
            return text
        }
    }
}

FNR == 1 {
    flush()
    file = FILENAME
    count = 0
    block_end = ""
    cut = 0
    pending_test_attribute = 0
}

cut { next }

kind == "hash" {
    line = trimmed($0)
    if (line != "" && substr(line, 1, 1) != "#") count++
    next
}

{
    line = without_leading_comments(trimmed($0))
    if (line == "") next
    if (kind == "rust" && substr(line, 1, 2) == "//") next
    if (kind == "sql" && substr(line, 1, 2) == "--") next
    if (kind == "rust" && skip_tests) {
        if ($0 ~ /^#\[cfg\(test\)\]/) { pending_test_attribute = 1; next }
        if (pending_test_attribute) {
            pending_test_attribute = 0
            if (line ~ /^(pub(\([a-z]+\))? )?mod [A-Za-z_0-9]+ *\{/) { cut = 1; next }
            if (line ~ /^(pub(\([a-z]+\))? )?mod [A-Za-z_0-9]+ *;/) next
            count++
        }
    }
    count++
}

END { flush() }
