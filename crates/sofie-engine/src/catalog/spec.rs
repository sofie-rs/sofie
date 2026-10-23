use super::Kind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentSpec {
    pub name: &'static str,
    pub summary: &'static str,
    pub children: Children,
    pub props: &'static [PropSpec],
    /// The engine feature an app enables to use it, if it is not always there.
    pub feature: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Children {
    None,
    Any,
    /// `Field` and `Submit`, in the order they are shown.
    FormParts,
    /// Rendered once per row, with the row named by `as`.
    RowContent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropSpec {
    pub name: &'static str,
    pub kind: PropKind,
    pub required: bool,
    pub summary: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropKind {
    /// Quoted text, `{…}`, or quoted text with `{…}` parts.
    Text,
    /// A path into the data, such as `{data.orders}`.
    Data,
    /// Quoted text without `{…}`.
    Literal,
    /// Quoted field names separated by spaces.
    Names,
    /// A URL posted to, quoted with `{…}` parts for its parameters.
    Post,
    /// One value: `{…}` or quoted text, not text with `{…}` parts.
    Value,
    /// Written bare, without a value.
    Flag,
    /// An element id: a letter, then letters, digits, `-` and `_`.
    Id,
    /// Named values, `{{name: value, …}}`.
    Fields,
}

impl PropKind {
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Text => "text, `{…}` or text with `{…}` parts",
            Self::Data => "a data path such as `{data.items}`",
            Self::Literal => "quoted text without `{…}`",
            Self::Names => "quoted field names separated by spaces",
            Self::Post => "a URL, quoted, with `{…}` parts for its parameters",
            Self::Value => "one value: `{…}` or quoted text",
            Self::Flag => "written bare, without a value",
            Self::Id => "an element id",
            Self::Fields => "`{{name: value, …}}`, each value a path, a string or an integer",
        }
    }
}

const fn prop(name: &'static str, kind: PropKind, summary: &'static str) -> PropSpec {
    PropSpec {
        name,
        kind,
        required: false,
        summary,
    }
}

const fn required(name: &'static str, kind: PropKind, summary: &'static str) -> PropSpec {
    PropSpec {
        name,
        kind,
        required: true,
        summary,
    }
}

const ID: PropSpec = prop(
    "id",
    PropKind::Id,
    "Names the element, so a fragment request or a target finds it.",
);
const TARGET: PropSpec = prop(
    "target",
    PropKind::Id,
    "The id of the element the answer replaces; without it the whole page is shown.",
);

const SECTION: ComponentSpec = ComponentSpec {
    name: "Section",
    summary: "A titled part of a page holding any markup and components.",
    children: Children::Any,
    props: &[required("title", PropKind::Text, "The heading."), ID],
    feature: None,
};

const TABLE: ComponentSpec = ComponentSpec {
    name: "Table",
    summary: "Rows of a list, one column per field; money fields are formatted. Its content, \
              if any, is rendered in a last cell of every row, such as an `Action`.",
    children: Children::RowContent,
    props: &[
        required(
            "id",
            PropKind::Id,
            "Names the table, so actions and listings replace it.",
        ),
        required("of", PropKind::Data, "The list of rows."),
        required(
            "columns",
            PropKind::Names,
            "The fields shown, in order; each header is the field's label.",
        ),
        prop(
            "as",
            PropKind::Literal,
            "Names the row for the content, such as `as=\"order\"` and then `{order.id}`.",
        ),
        prop(
            "link",
            PropKind::Text,
            "Where each row links to, with `{…}` parts naming the row, such as \
             `link=\"/orders/{order.id}\"` with `as=\"order\"`; the first column's cell becomes \
             the link.",
        ),
        prop(
            "sort",
            PropKind::Names,
            "Columns the viewer may sort by (needs the `listing` feature and a listing loader).",
        ),
        prop(
            "filter",
            PropKind::Names,
            "Fields the viewer may filter by (needs the `listing` feature and a listing loader).",
        ),
    ],
    feature: None,
};

const MONEY: ComponentSpec = ComponentSpec {
    name: "Money",
    summary: "An amount, `{minor, currency}`, formatted in the viewer's language.",
    children: Children::None,
    props: &[required("value", PropKind::Data, "The amount."), ID],
    feature: None,
};

const ACTION: ComponentSpec = ComponentSpec {
    name: "Action",
    summary: "A button posting to an action route, such as cancelling or booking the row it \
              sits in.",
    children: Children::None,
    props: &[
        required("post", PropKind::Post, "The action's URL."),
        required("label", PropKind::Text, "The button's text."),
        TARGET,
        prop(
            "when",
            PropKind::Value,
            "Shown only when this is truthy, such as `{order.status_to.CANCELLED}`.",
        ),
        prop(
            "with",
            PropKind::Fields,
            "Fields it posts, read by the route's input like a form's, such as \
             `{{slot: slot.id}}` for the row it sits in.",
        ),
        ID,
    ],
    feature: None,
};

const FORM: ComponentSpec = ComponentSpec {
    name: "Form",
    summary: "A form posting to an action route; its fields come from the route's input type \
              unless `Field`s name them.",
    children: Children::FormParts,
    props: &[
        required(
            "id",
            PropKind::Literal,
            "The form's element id, so errors are placed under its fields.",
        ),
        required("post", PropKind::Post, "The action's URL."),
        TARGET,
    ],
    feature: None,
};

const FIELD: ComponentSpec = ComponentSpec {
    name: "Field",
    summary: "One field of a `Form`, its control chosen from the input type's field.",
    children: Children::None,
    props: &[
        required("name", PropKind::Literal, "The input field it edits."),
        prop("label", PropKind::Text, "Replaces the field's label."),
        prop("value", PropKind::Value, "The value it starts with."),
        prop(
            "options",
            PropKind::Value,
            "A select's options: a list from the data, or a quoted registered enum name.",
        ),
        prop("currency", PropKind::Value, "A money field's currency."),
        prop(
            "required",
            PropKind::Flag,
            "Marks it required when the input type does not say so.",
        ),
    ],
    feature: None,
};

const SUBMIT: ComponentSpec = ComponentSpec {
    name: "Submit",
    summary: "The button sending a `Form`.",
    children: Children::None,
    props: &[required("label", PropKind::Text, "The button's text.")],
    feature: None,
};

const BADGE: ComponentSpec = ComponentSpec {
    name: "Badge",
    summary: "An enum value shown as its translated variant with the variant's tone.",
    children: Children::None,
    props: &[
        required("value", PropKind::Data, "The enum value."),
        required(
            "enum",
            PropKind::Literal,
            "The registered enum, such as `order_status`.",
        ),
        ID,
    ],
    feature: Some("i18n"),
};

const GRID: ComponentSpec = ComponentSpec {
    name: "Grid",
    summary: "Lays its children out in responsive columns.",
    children: Children::Any,
    props: &[ID],
    feature: Some("layout"),
};

const CARD: ComponentSpec = ComponentSpec {
    name: "Card",
    summary: "A boxed group of content, a link as a whole when it has `href`.",
    children: Children::Any,
    props: &[prop("href", PropKind::Text, "Where the card links to."), ID],
    feature: Some("layout"),
};

const IMAGE: ComponentSpec = ComponentSpec {
    name: "Image",
    summary: "An image, loaded lazily.",
    children: Children::None,
    props: &[
        required("src", PropKind::Text, "The image's URL."),
        required(
            "alt",
            PropKind::Text,
            "What the image shows, for screen readers.",
        ),
        ID,
    ],
    feature: Some("layout"),
};

const ISLAND: ComponentSpec = ComponentSpec {
    name: "Island",
    summary: "A Svelte component from the module's `islands/<name>.svelte`, mounted as a \
              custom element with typed props.",
    children: Children::None,
    props: &[
        required(
            "name",
            PropKind::Literal,
            "The island's name, lowercase with a hyphen.",
        ),
        required(
            "props",
            PropKind::Value,
            "The props, usually `{data}` or a part of it.",
        ),
        ID,
    ],
    feature: Some("islands"),
};

const MARKDOWN: ComponentSpec = ComponentSpec {
    name: "Markdown",
    summary: "Markdown rendered without raw HTML.",
    children: Children::None,
    props: &[
        required("text", PropKind::Data, "The markdown, from data."),
        ID,
    ],
    feature: Some("content"),
};

const HEAD: ComponentSpec = ComponentSpec {
    name: "Head",
    summary: "The page's title, description, canonical URL and link-preview tags, moved into \
              `<head>`.",
    children: Children::None,
    props: &[
        prop("title", PropKind::Text, "The page title."),
        prop(
            "description",
            PropKind::Text,
            "The description search engines show.",
        ),
        prop("image", PropKind::Text, "The link-preview image's URL."),
        prop("url", PropKind::Text, "The canonical URL."),
        prop(
            "type",
            PropKind::Text,
            "The Open Graph type, such as `article`.",
        ),
    ],
    feature: Some("content"),
};

/// Every component, whichever features this build enables.
pub const SPECS: &[ComponentSpec] = &[
    SECTION, TABLE, MONEY, ACTION, FORM, FIELD, SUBMIT, BADGE, GRID, CARD, IMAGE, ISLAND, MARKDOWN,
    HEAD,
];

impl Kind {
    pub(crate) fn spec(self) -> &'static ComponentSpec {
        SPECS
            .iter()
            .find(|spec| spec.name == self.name())
            .expect("every kind has a spec, as `every_kind_has_a_spec` checks")
    }
}

#[cfg(test)]
mod tests {
    use super::SPECS;
    use crate::catalog::Kind;

    #[test]
    fn every_kind_has_a_spec() {
        for kind in Kind::ALL {
            kind.spec();
        }
    }

    #[test]
    fn names_and_props_are_unique() {
        for (index, spec) in SPECS.iter().enumerate() {
            assert!(
                SPECS[..index].iter().all(|other| other.name != spec.name),
                "{}",
                spec.name
            );
            for (position, prop) in spec.props.iter().enumerate() {
                assert!(
                    spec.props[..position]
                        .iter()
                        .all(|other| other.name != prop.name),
                    "{}.{}",
                    spec.name,
                    prop.name
                );
            }
        }
    }
}
