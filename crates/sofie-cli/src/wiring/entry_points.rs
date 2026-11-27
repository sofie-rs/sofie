//! The app's own `lib.rs` and `main.rs` start the generated wiring, whose `app` and `worker`
//! take the app's flags once a module declares one.

use std::ops::Range;
use std::path::{Path, PathBuf};

use sofie_model::App;

use crate::gen_command::GenError;
use crate::rustfmt::format_rust;
use crate::source_edits::read;

const FLAGS_IMPORT: &str = "use sofie::flags::Flags;";
const WIRED_APP: &str = "wiring::app(";
const FLAGGED_ARGUMENT: &str = "flags";

/// Each pair starts the same app, without and with flags.
const RUNNERS: [(&str, &str); 3] = [
    ("run", "run_with_flags"),
    ("run_with_tasks", "run_with_flags_and_tasks"),
    ("run_worker", "run_worker_with_flags"),
];

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct EntryPointEdit {
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    pub(crate) found: String,
    pub(crate) expected: String,
    pub(crate) text: String,
}

pub(crate) fn entry_point_edits(app: &App) -> Result<Vec<EntryPointEdit>, GenError> {
    let flags = app.has_feature_flags();
    let source = app.app.path.join("src");
    let mut edits = Vec::new();
    let library = source.join("lib.rs");
    if library.is_file() {
        edits.extend(library_edit(&read(&library)?, flags, &library)?);
    }
    let main = source.join("main.rs");
    if main.is_file() {
        edits.extend(main_edit(&read(&main)?, flags, &main));
    }
    Ok(edits)
}

/// Only a call and a signature agreeing with each other are changed; anything else is the
/// author's own and left to the build.
fn library_edit(text: &str, flags: bool, file: &Path) -> Result<Option<EntryPointEdit>, GenError> {
    let Some(call) = Parenthesized::find(text, WIRED_APP) else {
        return Ok(None);
    };
    let Some(signature) = Parenthesized::find(text, "fn app(") else {
        return Ok(None);
    };
    let present = call.items.len();
    if present == usize::from(flags) + 1
        || present != signature.items.len()
        || !(1..=2).contains(&present)
    {
        return Ok(None);
    }
    let flags_parameter = format!("{FLAGGED_ARGUMENT}: Flags");
    let (arguments, parameters) = if flags {
        (
            vec![call.items[0], FLAGGED_ARGUMENT],
            vec![signature.items[0], flags_parameter.as_str()],
        )
    } else {
        (vec![call.items[0]], vec![signature.items[0]])
    };
    let line = line_of(text, call.span.start);
    let found = format!("{WIRED_APP}{})", call.items.join(", "));
    let expected = format!("{WIRED_APP}{})", arguments.join(", "));
    let mut replacements = [
        (call.span, expected.clone()),
        (signature.span, format!("fn app({})", parameters.join(", "))),
    ];
    // The later span first, so the earlier one's offsets still hold.
    replacements.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    let mut edited = text.to_owned();
    for (span, replacement) in replacements {
        edited.replace_range(span, &replacement);
    }
    let edited = if flags {
        with_flags_import(&edited)
    } else {
        edited.replacen(&format!("{FLAGS_IMPORT}\n"), "", 1)
    };
    Ok(Some(EntryPointEdit {
        file: file.to_path_buf(),
        line,
        found,
        expected,
        text: format_rust(&edited, file).map_err(GenError::Rustfmt)?,
    }))
}

fn main_edit(text: &str, flags: bool, file: &Path) -> Option<EntryPointEdit> {
    for (flagless, flagged) in RUNNERS {
        let (from, to) = if flags {
            (flagless, flagged)
        } else {
            (flagged, flagless)
        };
        let found = format!("sofie::{from}");
        let Some(start) = text.find(&format!("{found}(")) else {
            continue;
        };
        let expected = format!("sofie::{to}");
        let mut edited = text.to_owned();
        edited.replace_range(start..start + found.len(), &expected);
        return Some(EntryPointEdit {
            file: file.to_path_buf(),
            line: line_of(text, start),
            text: edited,
            found,
            expected,
        });
    }
    None
}

/// A call or signature: its span through the closing parenthesis and its comma separated items.
struct Parenthesized<'text> {
    span: Range<usize>,
    items: Vec<&'text str>,
}

impl<'text> Parenthesized<'text> {
    fn find(text: &'text str, opening: &str) -> Option<Self> {
        let start = text.find(opening)?;
        let list_start = start + opening.len();
        let list_end = list_start + text[list_start..].find(')')?;
        let list = &text[list_start..list_end];
        if list.contains('(') {
            return None;
        }
        let items = list
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .collect();
        Some(Self {
            span: start..list_end + 1,
            items,
        })
    }
}

/// Beside the first import, where rustfmt sorts it into the group.
fn with_flags_import(text: &str) -> String {
    let at = text
        .lines()
        .scan(0, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find(|(_, line)| line.starts_with("use "))
        .map_or(0, |(start, _)| start);
    format!("{}{FLAGS_IMPORT}\n{}", &text[..at], &text[at..])
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{library_edit, main_edit};

    const FLAGLESS_LIBRARY: &str = "use sofie::db::Db;\nuse sofie::prelude::*;\n\nmod wiring;\n\n\
         pub use wiring::{MODULES, worker};\n\npub async fn app(db: Db) -> Result<App> {\n    \
         wiring::app(db)\n}\n";
    const FLAGGED_LIBRARY: &str = "use sofie::db::Db;\nuse sofie::flags::Flags;\nuse sofie::prelude::*;\n\n\
         mod wiring;\n\npub use wiring::{MODULES, worker};\n\npub async fn app(db: Db, flags: Flags) -> \
         Result<App> {\n    wiring::app(db, flags)\n}\n";

    fn library(text: &str, flags: bool) -> Option<(usize, String, String, String)> {
        library_edit(text, flags, Path::new("app/src/lib.rs"))
            .unwrap()
            .map(|edit| (edit.line, edit.found, edit.expected, edit.text))
    }

    fn main(text: &str, flags: bool) -> Option<(String, String, String)> {
        main_edit(text, flags, Path::new("app/src/main.rs"))
            .map(|edit| (edit.found, edit.expected, edit.text))
    }

    #[test]
    fn hands_the_flags_to_the_wired_app_once_a_module_declares_one() {
        let (line, found, expected, text) = library(FLAGLESS_LIBRARY, true).unwrap();

        assert_eq!(
            (line, found.as_str(), expected.as_str()),
            (9, "wiring::app(db)", "wiring::app(db, flags)")
        );
        assert_eq!(text, FLAGGED_LIBRARY);
    }

    #[test]
    fn stops_handing_the_flags_once_no_module_declares_one() {
        let (_, found, expected, text) = library(FLAGGED_LIBRARY, false).unwrap();

        assert_eq!(
            (found.as_str(), expected.as_str()),
            ("wiring::app(db, flags)", "wiring::app(db)")
        );
        assert_eq!(text, FLAGLESS_LIBRARY);
    }

    #[test]
    fn keeps_an_app_whose_wired_call_matches_and_one_it_cannot_read() {
        assert_eq!(library(FLAGLESS_LIBRARY, false), None);
        assert_eq!(library(FLAGGED_LIBRARY, true), None);
        let disagreeing = FLAGLESS_LIBRARY.replace("app(db: Db)", "app(db: Db, extra: u8)");
        assert_eq!(library(&disagreeing, true), None);
    }

    #[test]
    fn rewrites_a_chained_wired_call_spread_over_lines() {
        let chained = FLAGGED_LIBRARY.replace(
            "    wiring::app(db, flags)\n",
            "    Ok(wiring::app(\n        db,\n        flags,\n    )?\n    .area(\"x\", x::routes))\n",
        );

        let (_, _, _, text) = library(&chained, false).unwrap();

        assert!(
            text.contains("Ok(wiring::app(db)?.area(\"x\", x::routes))"),
            "{text}"
        );
        assert!(!text.contains("Flags"), "{text}");
    }

    #[test]
    fn starts_the_app_through_the_runner_matching_its_flags() {
        let plain = "fn main() -> sofie::core::Result<()> {\n    sofie::run(shop::MODULES, shop::app, shop::worker)\n}\n";
        let flagged = plain.replace("sofie::run(", "sofie::run_with_flags(");

        let (found, expected, text) = main(plain, true).unwrap();
        assert_eq!(
            (found.as_str(), expected.as_str(), text.as_str()),
            ("sofie::run", "sofie::run_with_flags", flagged.as_str())
        );
        assert_eq!(main(&flagged, false).unwrap().2, plain);
        assert_eq!(main(plain, false), None);
        assert_eq!(main(&flagged, true), None);
    }

    #[test]
    fn keeps_the_tasks_when_switching_the_runner() {
        let tasks = "fn main() -> sofie::core::Result<()> {\n    sofie::run_with_tasks(a::MODULES, a::app, a::worker, a::tasks)\n}\n";

        let (_, expected, text) = main(tasks, true).unwrap();

        assert_eq!(expected, "sofie::run_with_flags_and_tasks");
        assert!(
            text.contains("sofie::run_with_flags_and_tasks(a::MODULES"),
            "{text}"
        );
    }
}
