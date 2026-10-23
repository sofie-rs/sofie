use super::attributes::parse_attributes;
use super::{Parser, Program};
use crate::syntax::{Attribute, Expression, LocatedError, Op, Path, Position, Root, Written};

impl<'s> Parser<'s> {
    /// Parsed on its own, then bound to the props it is given and the loops around it.
    pub(super) fn inline_component(
        &mut self,
        name: &str,
        source: &'s str,
        position: Position,
        program: &mut Program,
    ) -> Result<(), LocatedError> {
        if self.expanding.iter().any(|open| open == name) {
            bail!(
                position,
                "component `{name}` writes itself, so it would never end"
            );
        }
        let (given, is_self_closing) =
            parse_attributes(&mut self.cursor, &mut self.scope, name, position)?;
        if !is_self_closing {
            bail!(position, "`<{name}>` takes no children; end it with `/>`");
        }
        let mut expanding = self.expanding.clone();
        expanding.push(name.to_owned());
        let mut inner = Parser::new(source, self.components, expanding, self.depth);
        inner.is_in_form = self.is_in_form;
        inner.is_in_svg = self.is_in_svg;
        inner.names = self.names;
        inner.ids = std::mem::take(&mut self.ids);
        let parsed = inner.parse_all();
        self.ids = inner.ids;
        self.names = inner.names;
        let mut ops = parsed.map_err(|error| {
            LocatedError::new(
                position,
                format!(
                    "in component `{name}` at {}: {}",
                    error.position, error.message
                ),
            )
        })?;
        let props = Props {
            name,
            given: &given,
            offset: self.scope.depth(),
            position,
        };
        each_expression(&mut ops, &mut |expression| props.rebind(expression))?;
        each_expression::<LocatedError>(&mut ops, &mut |expression| {
            self.scope.record(expression, position);
            Ok(())
        })?;
        for op in ops {
            program.push(op);
        }
        Ok(())
    }
}

struct Props<'p> {
    name: &'p str,
    given: &'p [Attribute],
    /// The loop slots open where the component is written; its own come after them.
    offset: usize,
    position: Position,
}

impl Props<'_> {
    fn rebind(&self, expression: &mut Expression) -> Result<(), LocatedError> {
        match expression {
            Expression::Path(path) => {
                if let Some(bound) = self.bound(path)? {
                    *expression = bound;
                }
                Ok(())
            }
            Expression::Interpolated(parts) => {
                parts.iter_mut().try_for_each(|part| self.rebind(part))?;
                // A prop given as `"/orders/{row.id}"` is spliced in: a URL, an id or a
                // fragment search reads only one level of parts.
                *parts = std::mem::take(parts)
                    .into_iter()
                    .flat_map(|part| match part {
                        Expression::Interpolated(inner) => inner,
                        other => vec![other],
                    })
                    .collect();
                Ok(())
            }
            Expression::Call(call) => call
                .arguments_mut()
                .try_for_each(|argument| self.rebind(argument)),
            _ => Ok(()),
        }
    }

    /// `None` keeps the path, with its slot moved past the caller's.
    fn bound(&self, path: &mut Path) -> Result<Option<Expression>, LocatedError> {
        let Root::Data = path.root else {
            if let Root::Slot(slot) = &mut path.root {
                *slot += self.offset;
            }
            return Ok(None);
        };
        let name = self.name;
        let Some((prop, rest)) = path.fields.split_first() else {
            bail!(
                self.position,
                "component `{name}` reads `data` whole; read each prop as `data.<prop>`"
            );
        };
        let Some(given) = self.given.iter().find(|attribute| &attribute.name == prop) else {
            bail!(
                self.position,
                "component `{name}` reads `data.{prop}`, but `<{name}>` is not given `{prop}`"
            );
        };
        match (&given.value, rest) {
            (Expression::Path(outer), _) => {
                let mut fields = outer.fields.clone();
                fields.extend(rest.iter().cloned());
                Ok(Some(Expression::Path(Path {
                    root: outer.root,
                    fields,
                })))
            }
            (value, []) => Ok(Some(as_text(value.clone()))),
            (_, [field, ..]) => bail!(
                given.position,
                "`{prop}` is not a record, but component `{name}` reads its field `{field}`"
            ),
        }
    }
}

/// A quoted prop is text wherever the component writes it, so it cannot open an element the
/// template rules forbid.
fn as_text(value: Expression) -> Expression {
    match value {
        Expression::Authored(text) => Expression::Authored(text.replace('<', "&lt;")),
        Expression::Interpolated(parts) => {
            Expression::Interpolated(parts.into_iter().map(as_text).collect())
        }
        other => other,
    }
}

pub(crate) fn each_expression<E>(
    ops: &mut [Op<Written>],
    visit: &mut impl FnMut(&mut Expression) -> Result<(), E>,
) -> Result<(), E> {
    for op in ops {
        match op {
            Op::Markup(_) => {}
            Op::Text(expression) | Op::Url(expression) => visit(expression)?,
            Op::Identified { id, body } => {
                visit(id)?;
                each_expression(body, visit)?;
            }
            Op::Component(written) => {
                for attribute in &mut written.attributes {
                    visit(&mut attribute.value)?;
                }
                each_expression(&mut written.children, visit)?;
            }
            Op::If(conditional) => {
                visit(&mut conditional.condition)?;
                each_expression(&mut conditional.then_branch, visit)?;
                each_expression(&mut conditional.else_branch, visit)?;
            }
            Op::Each(each) => {
                visit(&mut each.list)?;
                each_expression(&mut each.body, visit)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{Components, Template};

    #[test]
    fn a_quoted_prop_written_as_text_opens_no_element() {
        let mut components = Components::default();
        components
            .add(
                "Teaser",
                "<p title=\"{data.title}\">{data.title}</p>".to_owned(),
            )
            .unwrap();
        let template = Template::parse_with(
            "screen",
            "<Teaser title=\"<script>x()</script> &amp; {data.name}\" />",
            &components,
        )
        .unwrap();

        let html = template.render(&json!({"name": "<b>"})).unwrap();

        assert_eq!(
            html,
            "<p title=\"&lt;script>x()&lt;/script> &amp; &lt;b&gt;\">\
&lt;script>x()&lt;/script> &amp; &lt;b&gt;</p>"
        );
    }

    #[test]
    fn a_prop_given_with_bound_parts_fills_a_link_and_a_fragment_id() {
        let mut components = Components::default();
        components
            .add(
                "Row",
                "<a id=\"{data.key}\" href=\"{data.link}\">x</a>".to_owned(),
            )
            .unwrap();
        let template = Template::parse_with(
            "screen",
            "{#each data.rows as row}<Row key=\"row-{row.id}\" link=\"/orders/{row.id}\" />{/each}",
            &components,
        )
        .unwrap();

        assert_eq!(
            template
                .render_fragment(&json!({"rows": [{"id": 7}]}), "row-7")
                .unwrap(),
            "<a id=\"row-7\" href=\"/orders/7\">x</a>"
        );
    }
}
