//! Only field names and app paths go into the source, which need no escaping.

use std::fmt::Write as _;

use sofie_values::Control;

use super::{Description, Look, Resource};

pub(super) struct Plan<'p> {
    pub(super) description: &'p Description,
    pub(super) at: String,
    pub(super) kept: &'p [Resource],
    pub(super) permission: Option<&'p str>,
}

pub(super) fn titled(verb: &str, name: &str) -> String {
    format!("{verb} {}", name.replace('_', " "))
}

pub(super) fn list_id(name: &str) -> String {
    format!("{}-list", name.replace('_', "-"))
}

impl Plan<'_> {
    pub(super) fn source(&self, screen: Resource) -> String {
        match screen {
            Resource::Index => self.index(),
            Resource::Show => self.show(),
            Resource::Create => self.create(),
            Resource::Edit => self.edit(),
            Resource::Delete => String::new(),
        }
    }

    fn title(&self, plural: bool) -> String {
        if plural {
            return super::plural_title(self.description.name);
        }
        sofie_engine::humanize(self.description.name)
    }

    fn title_of(&self, verb: &str) -> String {
        titled(verb, self.description.name)
    }

    fn keeps(&self, resource: Resource) -> bool {
        self.kept.contains(&resource)
    }

    fn list_path(&self) -> String {
        self.at.replace('{', "{data.")
    }

    fn gated(&self, action: &str, markup: &str) -> String {
        match self.permission {
            Some(prefix) => format!("{{#if can(\"{prefix}{action}\")}}{markup}{{/if}}"),
            None => markup.to_owned(),
        }
    }

    fn index(&self) -> String {
        let at = self.list_path();
        let description = self.description;
        let names: Vec<&str> = description.shown.iter().map(|field| field.name).collect();
        let columns = if names.is_empty() {
            "id".to_owned()
        } else {
            names.join(" ")
        };
        let mut html = format!("<Section title=\"{}\">", self.title(true));
        if self.keeps(Resource::Create) {
            let new = format!("<p><a href=\"{at}/new\">{}</a></p>", self.title_of("New"));
            html.push_str(&self.gated("create", &new));
        }
        let _ = write!(
            html,
            "<Table id=\"{}\" of={{data.list}} columns=\"{columns}\" as=\"row\"",
            list_id(self.description.name)
        );
        for (prop, names) in [
            ("sort", &description.sortable),
            ("filter", &description.filters),
        ] {
            if !names.is_empty() {
                let _ = write!(html, " {prop}=\"{}\"", names.join(" "));
            }
        }
        html.push('>');
        html.push_str(&self.row_links(&at));
        html.push_str("</Table></Section>");
        html
    }

    fn row_links(&self, at: &str) -> String {
        let row = format!("{at}/{{row.id}}");
        let mut links = String::new();
        if self.keeps(Resource::Show) {
            let _ = write!(links, "<a href=\"{row}\">Show</a> ");
        }
        if self.keeps(Resource::Edit) {
            links.push_str(&self.gated("update", &format!("<a href=\"{row}/edit\">Edit</a> ")));
        }
        if self.keeps(Resource::Delete) {
            let delete = format!(
                "<Action post=\"{row}/delete\" label=\"Delete\" target=\"{}\" />",
                list_id(self.description.name)
            );
            links.push_str(&self.gated("delete", &delete));
        }
        links
    }

    fn show(&self) -> String {
        let at = self.list_path();
        let mut html = format!(
            "<Section id=\"record\" title=\"{}\"><dl>",
            self.title(false)
        );
        for field in self.description.shown {
            let value = format!("data.record.{}", field.name);
            let shown = match field.look {
                Look::Money => format!("<Money value={{{value}}} />"),
                Look::Percentage | Look::Joined => format!("{{data.display.{}}}", field.name),
                Look::Parts(parts) => parts
                    .iter()
                    .map(|part| format!("{{{value}.{}}}", part.name))
                    .collect::<Vec<_>>()
                    .join(" "),
                Look::Text => format!("{{{value}}}"),
            };
            let _ = write!(
                html,
                "<dt>{}</dt><dd>{shown}</dd>",
                sofie_engine::humanize(field.name)
            );
        }
        html.push_str("</dl><p>");
        if self.keeps(Resource::Index) {
            let _ = write!(html, "<a href=\"{at}\">{}</a> ", self.title(true));
        }
        if self.keeps(Resource::Edit) {
            let edit = format!("<a href=\"{at}/{{data.record.id}}/edit\">Edit</a>");
            html.push_str(&self.gated("update", &edit));
        }
        html.push_str("</p></Section>");
        html.push_str(
            "{#if data.history}<Section title=\"History\"><Table id=\"history\" \
             of={data.history} columns=\"change fields actor action at\" /></Section>{/if}",
        );
        html
    }

    fn create(&self) -> String {
        format!(
            "<Section title=\"{}\"><Form id=\"create\" post=\"{}/new\">\
             <Submit label=\"Create\" /></Form></Section>",
            self.title_of("New"),
            self.list_path()
        )
    }

    /// A password or sensitive value is never sent back to the page.
    fn edit(&self) -> String {
        let mut html = format!(
            "<Section title=\"{}\"><Form id=\"edit\" post=\"{}/{{data.record.id}}/edit\">",
            self.title_of("Edit"),
            self.list_path()
        );
        for field in self.description.edited {
            if field.input.control == Control::Password {
                let _ = write!(html, "<Field name=\"{}\" />", field.name);
                continue;
            }
            let _ = write!(
                html,
                "<Field name=\"{0}\" value={{data.record.{0}}} />",
                field.name
            );
        }
        html.push_str("<Submit label=\"Save\" /></Form></Section>");
        html
    }
}
