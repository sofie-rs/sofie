use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};
use syn::{ItemImpl, ItemStruct, Visibility};

use crate::code::derives;
use crate::load::rust_files;
use crate::syntax::{last_name, line_of, parse_tokens, tokenize, type_last_name};
use crate::{EventDeclaration, ModelError};

const EVENT_TRAIT: &str = "Event";

/// Public `#[derive(Event)]` structs and `impl Event for …`, the events an app can name; only
/// files naming `Event` are parsed, so loading an app stays fast.
pub(crate) fn read_events(directory: &Path) -> Result<Vec<EventDeclaration>, ModelError> {
    let mut events = Vec::new();
    let mut hidden = HashSet::new();
    for file in rust_files(&directory.join("src"))? {
        let text = fs::read_to_string(&file).map_err(|error| {
            ModelError::in_file(&file, format!("cannot read the file: {error}"))
        })?;
        if !text.contains(EVENT_TRAIT) {
            continue;
        }
        let syntax = parse_tokens(&file, tokenize(&file, &text)?)?;
        let mut found = Events {
            file: &file,
            declared: &mut events,
            hidden: &mut hidden,
        };
        found.visit_file(&syntax);
    }
    events.retain(|event| !hidden.contains(&event.name));
    Ok(events)
}

struct Events<'a> {
    file: &'a PathBuf,
    declared: &'a mut Vec<EventDeclaration>,
    hidden: &'a mut HashSet<String>,
}

impl Events<'_> {
    fn push(&mut self, name: String, line: usize) {
        self.declared.push(EventDeclaration {
            name,
            file: self.file.clone(),
            line,
        });
    }
}

impl<'ast> Visit<'ast> for Events<'_> {
    fn visit_item_struct(&mut self, structure: &'ast ItemStruct) {
        if !matches!(structure.vis, Visibility::Public(_)) {
            self.hidden.insert(structure.ident.to_string());
            return;
        }
        if derives(&structure.attrs, EVENT_TRAIT) {
            self.push(structure.ident.to_string(), line_of(&structure.ident));
        }
    }

    fn visit_item_impl(&mut self, implementation: &'ast ItemImpl) {
        let implements_event = implementation
            .trait_
            .as_ref()
            .and_then(|(.., path, _)| last_name(path))
            .is_some_and(|name| name == EVENT_TRAIT);
        if implements_event && let Some(name) = type_last_name(&implementation.self_ty) {
            self.push(name, line_of(implementation));
        }
        visit::visit_item_impl(self, implementation);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::read_events;
    use crate::testing::TemporaryDirectory;

    fn names(package: &str) -> Vec<String> {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(package);
        let mut names: Vec<String> = read_events(&directory)
            .unwrap()
            .into_iter()
            .map(|event| event.name)
            .collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn derived_and_implemented_events_are_read_with_where_they_are_declared() {
        let directory = TemporaryDirectory::new("package-events");
        directory.write(
            "src/lib.rs",
            "mod inner {\n    #[derive(Clone, Event)]\n    pub struct Joined;\n}\n\
             pub struct Paid;\nimpl sofie::db::Event for Paid {}\nimpl Clone for Joined {}\n\
             pub(crate) struct Internal;\nimpl Event for Internal {}\n",
        );
        directory.write("src/plain.rs", "pub struct Other;\n");

        let events = read_events(directory.root()).unwrap();

        let found: Vec<(&str, usize)> = events
            .iter()
            .map(|event| (event.name.as_str(), event.line))
            .collect();
        assert_eq!(found, [("Joined", 3), ("Paid", 6)]);
        assert!(events[0].file.ends_with("src/lib.rs"));
    }

    #[test]
    fn the_framework_packages_raise_the_events_their_listeners_hear() {
        assert!(names("sofie-identity").contains(&"SignInLocked".to_owned()));
        assert!(names("sofie-identity").contains(&"AccountClosed".to_owned()));
        assert_eq!(
            names("sofie-tenancy"),
            [
                "MemberJoined",
                "MemberLeft",
                "MemberRoleChanged",
                "OrganizationDeleted"
            ]
        );
        assert_eq!(
            names("sofie-billing"),
            ["PurchaseCompleted", "SubscriptionChanged"]
        );
    }
}
