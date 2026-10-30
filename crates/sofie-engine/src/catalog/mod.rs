use sofie_values::FormField;

mod action;
#[cfg(feature = "i18n")]
mod badge;
#[cfg(feature = "content")]
pub(crate) mod content;
mod field;
mod form;
#[cfg(feature = "islands")]
mod island;
#[cfg(feature = "layout")]
pub(crate) mod layout;
#[cfg(feature = "listing")]
pub(crate) mod listing;
#[cfg(feature = "content")]
mod markdown;
mod money;
mod props;
mod section;
mod spec;
mod table;
mod target;

use self::action::{Action, build_action, render_action};
use self::field::build_field;
#[cfg(feature = "clients")]
pub(crate) use self::field::constant_label;
pub use self::field::field_errors;
pub(crate) use self::form::Form;
pub(crate) use self::form::FormPart;
pub use self::form::Submitted;
use self::form::{build_form, build_submit, render_form};
#[cfg(feature = "i18n")]
pub(crate) use self::money::money_of;
use self::money::{Money, build_money, render_money};
use self::props::Props;
pub use self::props::is_field_name;
use self::section::{Section, build_section, render_section};
pub use self::spec::{Children, ComponentSpec, PropKind, PropSpec, SPECS};
pub(crate) use self::table::Table;
use self::table::{build_table, render_table};
use crate::render::Renderer;
use crate::syntax::{Conditional, Expression, LocatedError, Loop, Op, Written};

macro_rules! kinds {
    ($($(#[$cfg:meta])* $kind:ident),*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum Kind {
            $($(#[$cfg])* $kind,)*
        }

        impl Kind {
            const ALL: &[Self] = &[$($(#[$cfg])* Self::$kind,)*];

            pub(crate) fn name(self) -> &'static str {
                match self {
                    $($(#[$cfg])* Self::$kind => stringify!($kind),)*
                }
            }
        }
    };
}

kinds!(
    Section,
    Table,
    Money,
    Action,
    Form,
    Field,
    Submit,
    #[cfg(feature = "i18n")]
    Badge,
    #[cfg(feature = "layout")]
    Grid,
    #[cfg(feature = "layout")]
    Card,
    #[cfg(feature = "layout")]
    Image,
    #[cfg(feature = "islands")]
    Island,
    #[cfg(feature = "content")]
    Markdown,
    #[cfg(feature = "content")]
    Head
);

impl Kind {
    pub(crate) fn named(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.name() == name)
    }

    pub(crate) fn list() -> String {
        let names: Vec<&str> = Self::ALL.iter().map(|kind| kind.name()).collect();
        names.join(", ")
    }

    fn takes_children(self) -> bool {
        self.spec().children != Children::None
    }
}

#[derive(Debug)]
pub(crate) struct Component {
    pub id: Option<Expression>,
    pub(crate) content: Content,
}

#[derive(Debug)]
pub(crate) enum Content {
    Section(Section),
    Table(Table),
    Money(Money),
    Action(Action),
    Form(Form),
    FormPart(FormPart),
    #[cfg(feature = "i18n")]
    Badge(badge::Badge),
    #[cfg(feature = "layout")]
    Layout(layout::Layout),
    #[cfg(feature = "islands")]
    Island(island::Island),
    #[cfg(feature = "content")]
    Published(content::Published),
}

impl Component {
    /// A table's row content is not among them: it exists once per row.
    pub(crate) fn children(&self) -> &[Op] {
        match &self.content {
            Content::Section(section) => &section.children,
            #[cfg(feature = "layout")]
            Content::Layout(layout) => layout.children(),
            _ => &[],
        }
    }
}

pub(crate) fn describe_forms(
    ops: &mut [Op],
    form_id: &str,
    fields: &'static [FormField],
) -> Result<bool, LocatedError> {
    let mut found = false;
    for op in ops {
        found |= match op {
            Op::Component(component) => match &mut component.content {
                Content::Form(form) if form.id == form_id => {
                    form.describe(fields)?;
                    true
                }
                content => describe_forms(content.children_mut(), form_id, fields)?,
            },
            Op::Identified { body, .. } | Op::Each(Loop { body, .. }) => {
                describe_forms(body, form_id, fields)?
            }
            Op::If(conditional) => {
                describe_forms(&mut conditional.then_branch, form_id, fields)?
                    | describe_forms(&mut conditional.else_branch, form_id, fields)?
            }
            Op::Markup(_) | Op::Text(_) | Op::Url(_) => false,
        };
    }
    Ok(found)
}

/// A form in each branch of an `{#if}` may share an id, so every one counts.
pub(crate) fn form_fields<'t>(ops: &'t [Op], form_id: &str, names: &mut Option<Vec<&'t str>>) {
    for op in ops {
        match op {
            Op::Component(component) => match &component.content {
                Content::Form(form) if form.id == form_id => {
                    let found = names.get_or_insert_with(Vec::new);
                    for name in form.field_names() {
                        if !found.contains(&name) {
                            found.push(name);
                        }
                    }
                }
                _ => form_fields(component.children(), form_id, names),
            },
            Op::Identified { body, .. } | Op::Each(Loop { body, .. }) => {
                form_fields(body, form_id, names);
            }
            Op::If(conditional) => {
                form_fields(&conditional.then_branch, form_id, names);
                form_fields(&conditional.else_branch, form_id, names);
            }
            Op::Markup(_) | Op::Text(_) | Op::Url(_) => {}
        }
    }
}

impl Content {
    fn children_mut(&mut self) -> &mut [Op] {
        match self {
            Self::Section(section) => &mut section.children,
            Self::Table(table) => &mut table.row_content,
            #[cfg(feature = "layout")]
            Self::Layout(layout) => layout.children_mut(),
            _ => &mut [],
        }
    }
}

pub(crate) fn build_all(ops: Vec<Op<Written>>) -> Result<Vec<Op>, LocatedError> {
    ops.into_iter().map(build_op).collect()
}

fn build_op(op: Op<Written>) -> Result<Op, LocatedError> {
    Ok(match op {
        Op::Markup(markup) => Op::Markup(markup),
        Op::Text(text) => Op::Text(text),
        Op::Url(url) => Op::Url(url),
        Op::Identified { id, body } => Op::Identified {
            id,
            body: build_all(body)?,
        },
        Op::Component(written) => Op::Component(build(written)?),
        Op::If(conditional) => Op::If(Conditional {
            condition: conditional.condition,
            then_branch: build_all(conditional.then_branch)?,
            else_branch: build_all(conditional.else_branch)?,
        }),
        Op::Each(each) => Op::Each(Loop {
            list: each.list,
            has_index: each.has_index,
            body: build_all(each.body)?,
        }),
    })
}

fn build(written: Written) -> Result<Component, LocatedError> {
    let Written {
        kind,
        attributes,
        children,
        position,
    } = written;
    let children = build_all(children)?;
    let mut props = Props::new(kind.spec(), attributes, position);
    if !kind.takes_children() {
        props.refuse_children(&children)?;
    }
    let content = match kind {
        Kind::Section => Content::Section(build_section(&mut props, children)?),
        Kind::Table => Content::Table(build_table(&mut props, children, position)?),
        Kind::Money => Content::Money(build_money(&mut props)?),
        Kind::Action => Content::Action(build_action(&mut props)?),
        Kind::Form => Content::Form(build_form(&mut props, children, position)?),
        Kind::Field => Content::FormPart(FormPart::Field(Box::new(build_field(
            &mut props, position,
        )?))),
        Kind::Submit => Content::FormPart(FormPart::Submit(build_submit(&mut props)?)),
        #[cfg(feature = "i18n")]
        Kind::Badge => Content::Badge(badge::build_badge(&mut props, position)?),
        #[cfg(feature = "layout")]
        Kind::Grid => Content::Layout(layout::Layout::Grid(children)),
        #[cfg(feature = "layout")]
        Kind::Card => Content::Layout(layout::build_card(&mut props, children, position)?),
        #[cfg(feature = "layout")]
        Kind::Image => Content::Layout(layout::build_image(&mut props)?),
        #[cfg(feature = "islands")]
        Kind::Island => Content::Island(island::build_island(&mut props)?),
        #[cfg(feature = "content")]
        Kind::Markdown => Content::Published(content::build_markdown(&mut props)?),
        #[cfg(feature = "content")]
        Kind::Head => Content::Published(content::build_head(&mut props)?),
    };
    let id = match &content {
        Content::Form(form) => Some(Expression::Authored(form.id.clone())),
        Content::Table(_) => Some(props.required("id")?),
        Content::FormPart(_) => None,
        #[cfg(feature = "content")]
        Content::Published(content::Published::Head(_)) => None,
        _ => props.optional("id"),
    };
    props.finish()?;
    Ok(Component { id, content })
}

pub(crate) fn render<'a>(
    component: &'a Component,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    let id = component.id.as_ref();
    match &component.content {
        Content::Section(section) => render_section(section, id, renderer)?,
        Content::Table(table) => render_table(table, id, renderer)?,
        Content::Money(money) => render_money(money, id, renderer),
        Content::Action(action) => render_action(action, id, renderer),
        #[cfg(feature = "i18n")]
        Content::Badge(badge) => badge::render_badge(badge, id, renderer),
        Content::Form(form) => render_form(form, renderer)?,
        #[cfg(feature = "layout")]
        Content::Layout(layout) => layout::render(layout, id, renderer)?,
        #[cfg(feature = "islands")]
        Content::Island(island) => island::render_island(island, id, renderer),
        #[cfg(feature = "content")]
        Content::Published(published) => content::render(published, id, renderer),
        // A part never stands alone: its `Form` took it while being built.
        Content::FormPart(_) => {}
    }
    Ok(())
}
