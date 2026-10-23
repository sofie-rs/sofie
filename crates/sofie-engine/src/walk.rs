#[cfg(feature = "content")]
use crate::catalog::content::Published;
use crate::catalog::{Component, Content, Form, FormPart};
use crate::syntax::{Conditional, Expression, Loop, Op};

/// What a template holds, visited without rendering it.
pub(crate) trait Visit<'t> {
    /// Each expression whole: a visitor looks into interpolations and calls itself.
    fn expression(&mut self, expression: &'t Expression);

    /// Read for whether it holds anything, not for what it holds: a condition, a list looped over.
    fn presence(&mut self, expression: &'t Expression) {
        self.expression(expression);
    }

    /// A list whose items are shown in turn: a loop's, or a table's rows.
    fn looped(&mut self, list: &'t Expression) {
        self.presence(list);
    }

    /// Around what is shown once per item of `list`, named by the next slot.
    fn enter_items(&mut self, list: &'t Expression, has_index: bool) {
        let _ = (list, has_index);
    }

    fn leave_items(&mut self, has_index: bool) {
        let _ = has_index;
    }

    /// Fields each item of `rows` is shown by.
    fn columns(&mut self, rows: &'t Expression, columns: &'t [String]) {
        let _ = (rows, columns);
    }

    fn form(&mut self, form: &'t Form) {
        let _ = form;
    }

    fn enumeration(&mut self, name: &'t str) {
        let _ = name;
    }

    /// Around what is shown only where `gate` holds.
    #[cfg(feature = "gates")]
    fn enter_gated(&mut self, gate: &'t crate::rendering::Gate) {
        let _ = gate;
    }

    #[cfg(feature = "gates")]
    fn leave_gated(&mut self) {}
}

pub(crate) fn walk<'t>(ops: &'t [Op], visitor: &mut impl Visit<'t>) {
    for op in ops {
        match op {
            Op::Markup(_) => {}
            Op::Text(expression) | Op::Url(expression) => visitor.expression(expression),
            Op::Identified { id, body } => {
                visitor.expression(id);
                walk(body, visitor);
            }
            Op::Component(component) => walk_component(component, visitor),
            Op::If(Conditional {
                condition,
                then_branch,
                else_branch,
            }) => {
                visitor.presence(condition);
                gated(condition, visitor, |visitor| walk(then_branch, visitor));
                walk(else_branch, visitor);
            }
            Op::Each(Loop {
                list,
                has_index,
                body,
            }) => {
                visitor.looped(list);
                visitor.enter_items(list, *has_index);
                walk(body, visitor);
                visitor.leave_items(*has_index);
            }
        }
    }
}

fn gated<'t, V: Visit<'t>>(
    condition: &'t Expression,
    visitor: &mut V,
    inside: impl FnOnce(&mut V),
) {
    #[cfg(feature = "gates")]
    if let Expression::Gate(gate) = condition {
        visitor.enter_gated(gate);
        inside(visitor);
        visitor.leave_gated();
        return;
    }
    let _ = condition;
    inside(visitor);
}

fn walk_component<'t>(component: &'t Component, visitor: &mut impl Visit<'t>) {
    if let Some(id) = &component.id {
        visitor.expression(id);
    }
    match &component.content {
        Content::Section(section) => {
            visitor.expression(&section.title);
            walk(&section.children, visitor);
        }
        Content::Table(table) => {
            visitor.looped(&table.rows);
            visitor.columns(&table.rows, &table.columns);
            visitor.enter_items(&table.rows, false);
            if let Some(link) = &table.link {
                visitor.expression(link);
            }
            walk(&table.row_content, visitor);
            visitor.leave_items(false);
            walk(&table.header, visitor);
            #[cfg(feature = "listing")]
            for (_, label) in &table.listing.filters {
                visitor.expression(label);
            }
        }
        Content::Money(money) => visitor.expression(&money.value),
        Content::Action(action) => {
            let shown = [&action.post, &action.label]
                .into_iter()
                .chain(action.target.as_ref().map(|target| &target.id))
                .chain(action.fields.iter().map(|field| &field.value));
            match &action.when {
                Some(when) => {
                    visitor.presence(when);
                    gated(when, visitor, |visitor| {
                        shown.for_each(|expression| visitor.expression(expression));
                    });
                }
                None => shown.for_each(|expression| visitor.expression(expression)),
            }
        }
        Content::Form(form) => {
            visitor.form(form);
            visitor.expression(&form.post);
            for part in &form.parts {
                walk_form_part(part, visitor);
            }
        }
        Content::FormPart(part) => walk_form_part(part, visitor),
        #[cfg(feature = "i18n")]
        Content::Badge(badge) => {
            visitor.expression(&badge.value);
            visitor.enumeration(&badge.enumeration);
        }
        #[cfg(feature = "layout")]
        Content::Layout(layout) => walk_layout(layout, visitor),
        #[cfg(feature = "islands")]
        Content::Island(island) => visitor.expression(&island.props),
        #[cfg(feature = "content")]
        Content::Published(published) => walk_published(published, visitor),
    }
}

#[cfg(feature = "content")]
fn walk_published<'t>(published: &'t Published, visitor: &mut impl Visit<'t>) {
    match published {
        Published::Markdown(text) => visitor.expression(text),
        Published::Head(head) => [
            &head.title,
            &head.description,
            &head.image,
            &head.url,
            &head.kind,
        ]
        .into_iter()
        .flatten()
        .for_each(|field| visitor.expression(field)),
    }
}

#[cfg(feature = "layout")]
fn walk_layout<'t>(layout: &'t crate::catalog::layout::Layout, visitor: &mut impl Visit<'t>) {
    use crate::catalog::layout::Layout;
    match layout {
        Layout::Grid(children) => walk(children, visitor),
        Layout::Card { href, children } => {
            if let Some(href) = href {
                visitor.expression(href);
            }
            walk(children, visitor);
        }
        Layout::Image { src, alt } => {
            visitor.expression(src);
            visitor.expression(alt);
        }
    }
}

fn walk_form_part<'t>(part: &'t FormPart, visitor: &mut impl Visit<'t>) {
    match part {
        FormPart::Submit(submit) => visitor.expression(&submit.label),
        FormPart::Field(field) => {
            visitor.expression(&field.label);
            for attribute in [&field.value, &field.options, &field.currency]
                .into_iter()
                .flatten()
            {
                visitor.expression(&attribute.value);
            }
            if let Some(Expression::Authored(enumeration)) =
                field.options.as_ref().map(|options| &options.value)
            {
                visitor.enumeration(enumeration);
            }
        }
    }
}
