use super::ids::posted_path;
use crate::TemplateError;
use crate::parse::parse_recorded;
use crate::slots::{Slots, Step};
use crate::syntax::{Attribute, Expression, Position};

/// Lets `sofie check` hold a template against the loader's type without rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub steps: Vec<Step>,
    pub line: usize,
    pub column: usize,
    /// The tag of the `<Island>` whose `props` the binding is.
    pub island: Option<String>,
}

/// An `<Action>`'s target and the input fields its `with` posts, where it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionPost {
    /// A bound path part is written `{}`, as in `/account/orders/{}/cancel`.
    pub post: String,
    pub fields: Vec<String>,
    pub line: usize,
}

/// A `Table`'s row link, where the table is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableLink {
    /// A bound path part is written `{}`, as in `/account/orders/{}`.
    pub link: String,
    pub line: usize,
}

/// An `entitled("…")` gate a template asks, where it is written.
#[cfg(feature = "gates")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskedEntitlement {
    pub name: String,
    pub line: usize,
}

pub fn bindings(name: &str, source: &str) -> Result<Vec<Binding>, TemplateError> {
    bindings_with(name, source, &crate::Components::default())
}

/// A component's paths are bound from where it is written, so they are the screen's.
pub fn bindings_with(
    name: &str,
    source: &str,
    components: &crate::Components,
) -> Result<Vec<Binding>, TemplateError> {
    parse_recorded(source, components)
        .map(|recorder| recorder.bindings)
        .map_err(|error| TemplateError::located(name, error))
}

pub fn action_posts_with(
    name: &str,
    source: &str,
    components: &crate::Components,
) -> Result<Vec<ActionPost>, TemplateError> {
    parse_recorded(source, components)
        .map(|recorder| recorder.actions)
        .map_err(|error| TemplateError::located(name, error))
}

pub fn table_links_with(
    name: &str,
    source: &str,
    components: &crate::Components,
) -> Result<Vec<TableLink>, TemplateError> {
    parse_recorded(source, components)
        .map(|recorder| recorder.links)
        .map_err(|error| TemplateError::located(name, error))
}

/// A component's gates are asked where it is written, so they are the screen's.
#[cfg(feature = "gates")]
pub fn asked_entitlements(
    name: &str,
    source: &str,
    components: &crate::Components,
) -> Result<Vec<AskedEntitlement>, TemplateError> {
    parse_recorded(source, components)
        .map(|recorder| recorder.entitlements)
        .map_err(|error| TemplateError::located(name, error))
}

#[derive(Default)]
pub(crate) struct Recorder {
    slots: Slots,
    bindings: Vec<Binding>,
    actions: Vec<ActionPost>,
    links: Vec<TableLink>,
    #[cfg(feature = "gates")]
    entitlements: Vec<AskedEntitlement>,
}

impl Recorder {
    pub(crate) fn push_items(&mut self, list: &Expression) {
        self.slots.push_items(list);
    }

    pub(crate) fn push_index(&mut self) {
        self.slots.push_dataless();
    }

    pub(crate) fn push_rows(&mut self, props: &[Attribute]) {
        match props.iter().find(|attribute| attribute.name == "of") {
            Some(rows) => self.slots.push_items(&rows.value),
            None => self.slots.push_dataless(),
        }
    }

    pub(crate) fn truncate(&mut self, depth: usize) {
        self.slots.truncate(depth);
    }

    pub(crate) fn record_action(&mut self, props: &[Attribute], position: Position) {
        let Some(post) = props.iter().find(|attribute| attribute.name == "post") else {
            return;
        };
        let fields = props
            .iter()
            .filter_map(|attribute| attribute.name.strip_prefix("with."))
            .map(str::to_owned)
            .collect();
        self.actions.push(ActionPost {
            post: posted_path(&post.value),
            fields,
            line: position.line,
        });
    }

    /// Each column is a field of a row of `of`, so it is bound like `{row.column}`.
    pub(crate) fn record_table_columns(&mut self, props: &[Attribute]) {
        let attribute = |name: &str| props.iter().find(|attribute| attribute.name == name);
        let (Some(rows), Some(columns)) = (attribute("of"), attribute("columns")) else {
            return;
        };
        let (Some(row), Expression::Authored(names)) =
            (self.slots.items_of(&rows.value), &columns.value)
        else {
            return;
        };
        for name in names.split_whitespace() {
            let mut steps = row.clone();
            steps.push(Step::Field(name.to_owned()));
            self.bindings.push(Binding {
                steps,
                line: columns.position.line,
                column: columns.position.column,
                island: None,
            });
        }
    }

    pub(crate) fn record_table_link(&mut self, props: &[Attribute], position: Position) {
        if let Some(link) = props.iter().find(|attribute| attribute.name == "link") {
            self.links.push(TableLink {
                link: posted_path(&link.value),
                line: position.line,
            });
        }
    }

    pub(crate) fn record(&mut self, expression: &Expression, position: Position) {
        match expression {
            Expression::Path(path) => {
                if let Some(steps) = self.slots.steps_of(path) {
                    self.bindings.push(Binding {
                        steps,
                        line: position.line,
                        column: position.column,
                        island: None,
                    });
                }
            }
            Expression::Interpolated(parts) => {
                for part in parts {
                    self.record(part, position);
                }
            }
            Expression::Call(call) => self.record_call(call, position),
            #[cfg(feature = "gates")]
            Expression::Gate(gate) if gate.asks == crate::rendering::Asks::Entitlement => {
                self.entitlements.push(AskedEntitlement {
                    name: gate.name.clone(),
                    line: position.line,
                });
            }
            _ => {}
        }
    }

    #[cfg(not(feature = "i18n"))]
    fn record_call(&mut self, call: &crate::language::Call, position: Position) {
        for value in call.values() {
            self.record(value, position);
        }
    }

    #[cfg(feature = "i18n")]
    fn record_call(&mut self, call: &crate::translations::Call, position: Position) {
        use crate::translations::Call;
        match call {
            Call::Translate { arguments, .. } => {
                for argument in arguments {
                    self.record(&argument.value, position);
                }
            }
            Call::Format { value, .. } => self.record(value, position),
            Call::Label { .. } => {}
        }
    }

    /// The attribute was recorded as it was read, before the component knew its kind.
    #[cfg(feature = "islands")]
    pub(crate) fn mark_island(&mut self, tag: &str, props: Position) {
        for binding in &mut self.bindings {
            if (binding.line, binding.column) == (props.line, props.column) {
                binding.island = Some(tag.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::bindings;
    use crate::slots::Step;

    fn paths(source: &str) -> Vec<String> {
        bindings("screen", source)
            .unwrap()
            .iter()
            .map(|binding| {
                let steps: Vec<&str> = binding
                    .steps
                    .iter()
                    .map(|step| match step {
                        Step::Field(name) => name.as_str(),
                        Step::Item => "[]",
                    })
                    .collect();
                steps.join(".")
            })
            .collect()
    }

    #[test]
    fn expands_loop_and_row_names_to_paths_from_the_data_root() {
        let source = "<p title=\"{data.title}\">{data.count}</p>\
            {#each data.orders as order, index}{order.id}{index}{/each}\
            <Table id=\"t\" of={data.lines} columns=\"name\" as=\"line\">\
            <Action post=\"/x/{line.id}\" label=\"x\" when={line.status_to.PAID} /></Table>";

        assert_eq!(
            paths(source),
            [
                "title",
                "count",
                "orders",
                "orders.[].id",
                "lines",
                "lines.[].name",
                "lines.[].id",
                "lines.[].status_to.PAID",
            ]
        );
    }

    #[test]
    fn binds_an_action_s_posted_fields() {
        let source = "<Table id=\"t\" of={data.slots} columns=\"time\" as=\"slot\">\
            <Action post=\"/slots/book\" label=\"Book\" with={{slot: slot.id, by: data.user}} />\
            </Table>";

        assert_eq!(
            paths(source),
            ["slots", "slots.[].time", "slots.[].id", "user"]
        );
    }

    #[test]
    fn lists_each_action_s_target_and_posted_fields_with_its_line() {
        use super::{ActionPost, action_posts_with};
        let source = "<Action post=\"/orders/{data.id}/pick\" label=\"x\" />\n\
            <Action post=\"/slots/book\" label=\"Book\" with={{slot: data.slot, by: 3}} />";

        let posts = action_posts_with("screen", source, &crate::Components::default()).unwrap();

        let at = |post: &str, fields: &[&str], line| ActionPost {
            post: post.to_owned(),
            fields: fields.iter().map(|field| (*field).to_owned()).collect(),
            line,
        };
        assert_eq!(
            posts,
            [
                at("/orders/{}/pick", &[], 1),
                at("/slots/book", &["slot", "by"], 2)
            ]
        );
    }

    #[test]
    fn lists_each_table_s_row_link_with_its_line() {
        use super::{TableLink, table_links_with};
        let source = "<Table id=\"a\" of={data.orders} columns=\"id\" as=\"order\" \
            link=\"/account/orders/{order.id}\" />\n\
            <Table id=\"b\" of={data.lines} columns=\"name\" />";

        let links = table_links_with("screen", source, &crate::Components::default()).unwrap();

        assert_eq!(
            links,
            [TableLink {
                link: "/account/orders/{}".to_owned(),
                line: 1
            }]
        );
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn follows_paths_into_translation_and_format_arguments() {
        let source = "<p>{t(\"order.total\", amount = data.total)}{number(data.count)}</p>";

        assert_eq!(paths(source), ["total", "count"]);
    }

    #[cfg(feature = "gates")]
    #[test]
    fn lists_each_entitlement_a_template_asks_with_its_line() {
        use super::{AskedEntitlement, asked_entitlements};
        let source = "<p>entitled(\"x.y\")</p>\n{#if entitled(\"project.export\")}<p>Export</p>\
            {/if}{#if can(\"order.order.*.read\")}<p>Read</p>{/if}\n\
            <Action post=\"/api\" label=\"Api\" when={entitled(\"project.api\")} />";

        let asked = asked_entitlements("screen", source, &crate::Components::default()).unwrap();

        let at = |name: &str, line| AskedEntitlement {
            name: name.to_owned(),
            line,
        };
        assert_eq!(asked, [at("project.export", 2), at("project.api", 3)]);
    }

    #[cfg(feature = "islands")]
    #[test]
    fn marks_the_props_an_island_is_given() {
        let source = "{#each data.charts as chart}\
            <Island name=\"order-chart\" props={chart.points} />{/each}<p>{data.title}</p>";

        let marked: Vec<(Option<String>, usize)> = bindings("screen", source)
            .unwrap()
            .into_iter()
            .map(|binding| (binding.island, binding.steps.len()))
            .collect();

        assert_eq!(
            marked,
            [(None, 1), (Some("order-chart".to_owned()), 3), (None, 1)]
        );
    }
}
