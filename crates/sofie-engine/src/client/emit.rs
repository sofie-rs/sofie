use std::fmt::Write;

use super::script::{array, literal, slot, text, url_text, value};
use super::tree::{AttributePart, Element, ElementAttribute, Node};
use crate::catalog::{Component, Content, FormPart};
use crate::syntax::{Expression, Position};

pub(super) struct Emitter {
    out: String,
    /// Loop and row names bound around the node being written.
    depth: usize,
}

impl Emitter {
    pub(super) fn new() -> Self {
        Self {
            out: String::new(),
            depth: 0,
        }
    }

    pub(super) fn screen(mut self, nodes: &[Node<'_>]) -> Result<String, String> {
        self.out.push_str(
            "<script>\nimport { Catalog, S } from \"sofie:svelte\";\n\
             let { data, view } = $props();\n</script>\n",
        );
        self.nodes(nodes)?;
        self.out.push('\n');
        Ok(self.out)
    }

    fn nodes(&mut self, nodes: &[Node<'_>]) -> Result<(), String> {
        nodes.iter().try_for_each(|node| self.node(node))
    }

    fn node(&mut self, node: &Node<'_>) -> Result<(), String> {
        match node {
            Node::Element(element) => self.element(element),
            Node::Markup(markup) => {
                self.out.push_str(&svelte_markup(markup));
                Ok(())
            }
            Node::Text(expression) => {
                self.braced(&text(expression));
                Ok(())
            }
            Node::Component(component) => self.component(component),
            Node::If {
                condition,
                then_branch,
                else_branch,
            } => self.conditional(condition, then_branch, else_branch),
            Node::Each {
                list,
                has_index,
                body,
            } => self.each(list, *has_index, body),
        }
    }

    fn braced(&mut self, code: &str) {
        self.out.push('{');
        self.out.push_str(code);
        self.out.push('}');
    }

    /// Svelte writes into the head only through `<svelte:head>`.
    fn element(&mut self, element: &Element<'_>) -> Result<(), String> {
        if element.tag == "title" {
            self.out.push_str("<svelte:head>");
            self.plain_element(element)?;
            self.out.push_str("</svelte:head>");
            return Ok(());
        }
        self.plain_element(element)
    }

    fn plain_element(&mut self, element: &Element<'_>) -> Result<(), String> {
        self.out.push('<');
        self.out.push_str(&element.tag);
        for attribute in &element.attributes {
            self.attribute(attribute);
        }
        let Some(children) = &element.children else {
            self.out.push('>');
            return Ok(());
        };
        self.out.push('>');
        self.nodes(children)?;
        self.out.push_str("</");
        self.out.push_str(&element.tag);
        self.out.push('>');
        Ok(())
    }

    fn attribute(&mut self, attribute: &ElementAttribute<'_>) {
        self.out.push(' ');
        self.out.push_str(&attribute.name);
        self.out.push_str("=\"");
        for part in &attribute.value {
            match part {
                AttributePart::Markup(markup) => self.out.push_str(&svelte_markup(markup)),
                AttributePart::Text(expression) => self.braced(&text(expression)),
                AttributePart::Url(expression) => {
                    self.braced(&format!("S.url({})", url_text(expression)));
                }
            }
        }
        self.out.push('"');
    }

    fn conditional(
        &mut self,
        condition: &Expression,
        then_branch: &[Node<'_>],
        else_branch: &[Node<'_>],
    ) -> Result<(), String> {
        let condition = format!("S.truthy({})", value(condition));
        self.out.push_str("{#if ");
        self.out.push_str(&condition);
        self.out.push('}');
        self.nodes(then_branch)?;
        if !else_branch.is_empty() {
            self.out.push_str("{:else}");
            self.nodes(else_branch)?;
        }
        self.out.push_str("{/if}");
        Ok(())
    }

    fn each(
        &mut self,
        list: &Expression,
        has_index: bool,
        body: &[Node<'_>],
    ) -> Result<(), String> {
        let item = slot(self.depth);
        let index = if has_index {
            slot(self.depth + 1)
        } else {
            format!("k{}", self.depth)
        };
        let list = format!("S.items({})", value(list));
        let outer = self.depth;
        self.depth += 1 + usize::from(has_index);
        let _ = write!(self.out, "{{#each {list} as {item}, {index}}}");
        self.nodes(body)?;
        self.out.push_str("{/each}");
        self.depth = outer;
        Ok(())
    }

    fn component(&mut self, component: &Component) -> Result<(), String> {
        let call = ComponentCall::of(component, self.depth)?;
        let props: Vec<String> = call
            .props
            .iter()
            .map(|(name, code)| format!("{}: {code}", literal(name)))
            .collect();
        self.out.push_str("<Catalog {view} name=");
        self.out.push_str(&literal(call.name));
        self.out.push_str(" props={{");
        self.out.push_str(&props.join(", "));
        self.out.push_str("}}");
        if call.children.is_none() && call.row.is_none() {
            self.out.push_str(" />");
            return Ok(());
        }
        self.out.push('>');
        if let Some(row) = call.row {
            self.row(row)?;
        }
        if let Some(children) = call.children {
            let nodes = super::tree::tree(children)?;
            self.nodes(&nodes)?;
        }
        self.out.push_str("</Catalog>");
        Ok(())
    }

    /// A table's row content as a snippet, given the row as the next slot.
    fn row(&mut self, ops: &[crate::syntax::Op]) -> Result<(), String> {
        let nodes = super::tree::tree(ops)?;
        let row = slot(self.depth);
        self.depth += 1;
        let _ = write!(self.out, "{{#snippet row({row})}}");
        self.nodes(&nodes)?;
        self.out.push_str("{/snippet}");
        self.depth -= 1;
        Ok(())
    }
}

struct ComponentCall<'c> {
    name: &'static str,
    props: Vec<(&'static str, String)>,
    children: Option<&'c [crate::syntax::Op]>,
    row: Option<&'c [crate::syntax::Op]>,
}

impl<'c> ComponentCall<'c> {
    /// `depth` is the slot a table's row is bound to.
    fn of(component: &'c Component, depth: usize) -> Result<Self, String> {
        let mut call = Self {
            name: "",
            props: Vec::new(),
            children: None,
            row: None,
        };
        if let Some(id) = &component.id {
            call.props.push(("id", text(id)));
        }
        match &component.content {
            Content::Section(section) => {
                call.name = "Section";
                call.props.push(("title", text(&section.title)));
                call.children = Some(&section.children);
            }
            Content::Table(table) => call.table(table, depth),
            Content::Money(money) => {
                call.name = "Money";
                call.props.push(("value", value(&money.value)));
            }
            Content::Action(action) => {
                call.name = "Action";
                call.props.push(("post", url_text(&action.post)));
                call.props.push(("label", text(&action.label)));
                if let Some(target) = &action.target {
                    call.props.push(("target", text(&target.id)));
                }
                if let Some(when) = &action.when {
                    call.props.push(("when", value(when)));
                }
                if !action.fields.is_empty() {
                    let fields: Vec<String> = action
                        .fields
                        .iter()
                        .map(|field| format!("{}: {}", literal(&field.name), value(&field.value)))
                        .collect();
                    call.props
                        .push(("with", format!("{{{}}}", fields.join(", "))));
                }
            }
            Content::Form(form) => {
                call.name = "Form";
                call.props.retain(|(name, _)| *name != "id");
                call.props.push(("id", literal(&form.id)));
                call.props.push(("post", url_text(&form.post)));
                if let Some(target) = &form.target {
                    call.props.push(("target", text(&target.id)));
                }
                call.props
                    .push(("parts", array(form.parts.iter().map(form_part))));
            }
            Content::FormPart(_) => return Err("a form part outside its form".to_owned()),
            #[cfg(feature = "i18n")]
            Content::Badge(badge) => {
                call.name = "Badge";
                call.props.push(("value", value(&badge.value)));
                call.props.push(("enum", literal(&badge.enumeration)));
            }
            #[cfg(feature = "layout")]
            Content::Layout(layout) => call.layout(layout),
            #[cfg(feature = "islands")]
            Content::Island(island) => {
                call.name = "Island";
                call.props.push(("name", literal(&island.tag)));
                call.props.push(("props", value(&island.props)));
            }
            #[cfg(feature = "content")]
            Content::Published(_) => {
                return Err(
                    "`Markdown` and `Head` render only in HTML areas; keep written pages in an \
                     area without a client"
                        .to_owned(),
                );
            }
        }
        Ok(call)
    }

    fn table(&mut self, table: &'c crate::catalog::Table, depth: usize) {
        self.name = "Table";
        self.props.push(("rows", value(&table.rows)));
        self.props.push((
            "columns",
            array(table.columns.iter().map(|name| literal(name))),
        ));
        let labels = table
            .columns
            .iter()
            .map(|name| text(&crate::language::label(name, Position::START)));
        self.props.push(("labels", array(labels)));
        if let Some(link) = &table.link {
            let row = slot(depth);
            self.props
                .push(("link", format!("({row}) => {}", url_text(link))));
        }
        #[cfg(feature = "listing")]
        if let Some(listing) = listing(&table.listing) {
            self.props.push(("listing", listing));
        }
        if !table.row_content.is_empty() {
            self.row = Some(&table.row_content);
        }
    }

    #[cfg(feature = "layout")]
    fn layout(&mut self, layout: &'c crate::catalog::layout::Layout) {
        use crate::catalog::layout::Layout;
        match layout {
            Layout::Grid(children) => {
                self.name = "Grid";
                self.children = Some(children);
            }
            Layout::Card { href, children } => {
                self.name = "Card";
                if let Some(href) = href {
                    self.props.push(("href", url_text(href)));
                }
                self.children = Some(children);
            }
            Layout::Image { src, alt } => {
                self.name = "Image";
                self.props.push(("src", url_text(src)));
                self.props.push(("alt", text(alt)));
            }
        }
    }
}

#[cfg(feature = "listing")]
fn listing(listing: &crate::catalog::listing::Listing) -> Option<String> {
    if listing.sortable.is_empty() && listing.filters.is_empty() {
        return None;
    }
    let sort = array(listing.sortable.iter().map(|name| literal(name)));
    let filter = array(listing.filters.iter().map(|(name, label)| {
        format!(
            "{{\"name\": {}, \"label\": {}}}",
            literal(name),
            text(label)
        )
    }));
    Some(format!("{{\"sort\": {sort}, \"filter\": {filter}}}"))
}

fn form_part(part: &FormPart) -> String {
    match part {
        FormPart::Submit(submit) => format!("{{\"submit\": {}}}", text(&submit.label)),
        FormPart::Field(field) => {
            let mut props = vec![
                format!("\"name\": {}", literal(&field.name)),
                format!("\"label\": {}", text(&field.label)),
            ];
            for (name, attribute) in [
                ("value", &field.value),
                ("options", &field.options),
                ("currency", &field.currency),
            ] {
                if let Some(attribute) = attribute {
                    props.push(format!("{}: {}", literal(name), value(&attribute.value)));
                }
            }
            if field.is_written_required() {
                props.push("\"required\": true".to_owned());
            }
            format!("{{\"field\": {{{}}}}}", props.join(", "))
        }
    }
}

fn svelte_markup(markup: &str) -> String {
    markup.replace('{', "&#123;").replace('}', "&#125;")
}
