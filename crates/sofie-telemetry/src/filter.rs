use tracing::Metadata;
use tracing::level_filters::LevelFilter;

use crate::InitError;
use crate::init::variable_or;

const LOG_VARIABLE: &str = "SOFIE_LOG";
const UNSET_DIRECTIVES: &str = "info";
const GRAMMAR: &str = "a comma-separated list of `level` or `target=level`, e.g. \
                       `info,sofie_http=debug`, with levels trace, debug, info, warn, error, off";

/// A bare level applies to targets no `target=level` names; the longest matching target decides.
#[derive(Debug, PartialEq)]
pub(crate) struct LogFilter {
    default: LevelFilter,
    targets: Vec<TargetLevel>,
}

#[derive(Debug, PartialEq)]
struct TargetLevel {
    target: String,
    level: LevelFilter,
}

impl LogFilter {
    pub(crate) fn from_environment() -> Result<Self, InitError> {
        Self::parse(&variable_or(LOG_VARIABLE, UNSET_DIRECTIVES)?)
    }

    pub(crate) fn parse(directives: &str) -> Result<Self, InitError> {
        let mut default = None;
        let mut targets: Vec<TargetLevel> = Vec::new();
        for directive in directives.split(',').map(str::trim) {
            let Some((target, level)) = directive.split_once('=') else {
                let level = parse_level(directive)?;
                if default.replace(level).is_some() {
                    return Err(refusal(directive, "a second bare level"));
                }
                continue;
            };
            let target = parse_target(target.trim(), directive)?;
            if targets.iter().any(|existing| existing.target == target) {
                return Err(refusal(directive, "a target named twice"));
            }
            let level = parse_level(level.trim())?;
            targets.push(TargetLevel { target, level });
        }
        targets.sort_by_key(|named| std::cmp::Reverse(named.target.len()));
        Ok(Self {
            default: default.unwrap_or(LevelFilter::OFF),
            targets,
        })
    }

    pub(crate) fn allows(&self, metadata: &Metadata<'_>) -> bool {
        *metadata.level() <= self.level_of(metadata.target())
    }

    pub(crate) fn max_level(&self) -> LevelFilter {
        self.targets
            .iter()
            .map(|named| named.level)
            .fold(self.default, LevelFilter::max)
    }

    fn level_of(&self, target: &str) -> LevelFilter {
        self.targets
            .iter()
            .find(|named| covers(&named.target, target))
            .map_or(self.default, |named| named.level)
    }
}

fn covers(named: &str, target: &str) -> bool {
    target
        .strip_prefix(named)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

fn parse_level(text: &str) -> Result<LevelFilter, InitError> {
    match text.to_ascii_lowercase().as_str() {
        "off" => Ok(LevelFilter::OFF),
        "error" => Ok(LevelFilter::ERROR),
        "warn" => Ok(LevelFilter::WARN),
        "info" => Ok(LevelFilter::INFO),
        "debug" => Ok(LevelFilter::DEBUG),
        "trace" => Ok(LevelFilter::TRACE),
        _ => Err(refusal(text, "not a level")),
    }
}

fn parse_target(target: &str, directive: &str) -> Result<String, InitError> {
    let well_formed = target.split("::").all(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
    });
    if !well_formed {
        return Err(refusal(directive, "not a target=level"));
    }
    Ok(target.to_owned())
}

fn refusal(text: &str, problem: &str) -> InitError {
    InitError::new(format!(
        "{LOG_VARIABLE} is not a filter: `{text}` is {problem}; expected {GRAMMAR}"
    ))
}

#[cfg(test)]
mod tests {
    use tracing::Level;
    use tracing::level_filters::LevelFilter;

    use super::LogFilter;

    fn level_of(directives: &str, target: &str) -> LevelFilter {
        LogFilter::parse(directives).unwrap().level_of(target)
    }

    #[test]
    fn a_bare_level_applies_to_every_target_not_named() {
        assert_eq!(level_of("warn", "sofie_http::trace"), LevelFilter::WARN);
        assert_eq!(level_of(" DEBUG ", "hyper"), LevelFilter::DEBUG);
    }

    #[test]
    fn the_longest_named_target_decides_and_covers_its_submodules() {
        let directives = "info, sofie_http=debug, sofie_http::trace=off, sofie_db = warn";

        assert_eq!(level_of(directives, "sofie_http"), LevelFilter::DEBUG);
        assert_eq!(
            level_of(directives, "sofie_http::router"),
            LevelFilter::DEBUG
        );
        assert_eq!(level_of(directives, "sofie_http::trace"), LevelFilter::OFF);
        assert_eq!(
            level_of(directives, "sofie_db::messages"),
            LevelFilter::WARN
        );
        assert_eq!(level_of(directives, "sofie_http_extra"), LevelFilter::INFO);
        assert_eq!(level_of(directives, "shop"), LevelFilter::INFO);
    }

    #[test]
    fn without_a_bare_level_only_named_targets_log() {
        assert_eq!(level_of("sofie_http=debug", "sofie_db"), LevelFilter::OFF);
        assert_eq!(
            level_of("sofie_http=debug", "sofie_http"),
            LevelFilter::DEBUG
        );
    }

    #[test]
    fn the_max_level_is_the_most_verbose_directive() {
        let filter = LogFilter::parse("warn,sofie_db=trace").unwrap();

        assert_eq!(filter.max_level(), LevelFilter::TRACE);
        assert!(Level::TRACE <= filter.max_level());
    }

    #[test]
    fn anything_else_is_refused_with_the_grammar() {
        for directives in [
            "",
            "info,",
            "verbose",
            "sofie_http",
            "sofie_http=loud",
            "sofie_http=",
            "=debug",
            "sofie-http=debug",
            "sofie_http:trace=debug",
            "sofie_http::=debug",
            "sofie_http[span]=debug",
            "sofie_http=debug,sofie_http=warn",
            "info,warn",
            "info,sofie_http=debug=trace",
            "3",
        ] {
            let refused = LogFilter::parse(directives).unwrap_err().to_string();
            assert!(
                refused.starts_with("SOFIE_LOG is not a filter: `"),
                "{directives}: {refused}"
            );
            assert!(refused.contains("target=level"), "{directives}: {refused}");
        }
    }
}
