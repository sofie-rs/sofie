use super::Call;
#[cfg(feature = "content")]
use crate::catalog::content::Published;
#[cfg(feature = "layout")]
use crate::catalog::layout::Layout;
use crate::catalog::{Component, Content, FormPart};
use crate::syntax::{Expression, LocatedError, Op, Position};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyUse {
    pub key: String,
    pub kind: KeyKind,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Message { arguments: Vec<String> },
    Label,
    Enum,
}

/// Calls write text, so one where a value, a URL or an id is read is refused.
pub(crate) fn key_uses(ops: &[Op]) -> Result<Vec<KeyUse>, LocatedError> {
    let mut walker = Walker { uses: Vec::new() };
    walker.ops(ops)?;
    Ok(walker.uses)
}

struct Walker {
    uses: Vec<KeyUse>,
}

impl Walker {
    fn ops(&mut self, ops: &[Op]) -> Result<(), LocatedError> {
        for op in ops {
            match op {
                Op::Markup(_) => {}
                Op::Text(text) => self.text(text),
                Op::Url(url) => refuse_call(url)?,
                Op::Identified { id, body } => {
                    refuse_call(id)?;
                    self.ops(body)?;
                }
                Op::Component(component) => self.component(component)?,
                Op::If(conditional) => {
                    refuse_call(&conditional.condition)?;
                    self.ops(&conditional.then_branch)?;
                    self.ops(&conditional.else_branch)?;
                }
                Op::Each(each) => {
                    refuse_call(&each.list)?;
                    self.ops(&each.body)?;
                }
            }
        }
        Ok(())
    }

    fn component(&mut self, component: &Component) -> Result<(), LocatedError> {
        refuse_calls([component.id.as_ref()])?;
        match &component.content {
            Content::Section(section) => {
                self.text(&section.title);
                self.ops(&section.children)
            }
            Content::Table(table) => {
                refuse_calls([table.link.as_ref()])?;
                self.ops(&table.header)?;
                self.ops(&table.row_content)
            }
            Content::Money(_) => Ok(()),
            Content::Action(action) => {
                self.text(&action.label);
                refuse_calls([
                    Some(&action.post),
                    action.when.as_ref(),
                    action.target.as_ref().map(|target| &target.id),
                ])
            }
            Content::Form(form) => {
                refuse_calls([
                    Some(&form.post),
                    form.target.as_ref().map(|target| &target.id),
                ])?;
                form.parts.iter().try_for_each(|part| self.form_part(part))
            }
            Content::FormPart(part) => self.form_part(part),
            #[cfg(feature = "layout")]
            Content::Layout(Layout::Grid(children)) => self.ops(children),
            #[cfg(feature = "layout")]
            Content::Layout(Layout::Card { href, children }) => {
                refuse_calls([href.as_ref()])?;
                self.ops(children)
            }
            #[cfg(feature = "layout")]
            Content::Layout(Layout::Image { src, alt }) => {
                self.text(alt);
                refuse_call(src)
            }
            Content::Badge(badge) => {
                self.enumeration(&badge.enumeration, badge.position);
                Ok(())
            }
            #[cfg(feature = "islands")]
            Content::Island(island) => refuse_call(&island.props),
            #[cfg(feature = "content")]
            Content::Published(Published::Markdown(text)) => refuse_call(text),
            #[cfg(feature = "content")]
            Content::Published(Published::Head(head)) => {
                for text in [&head.title, &head.description, &head.kind]
                    .into_iter()
                    .flatten()
                {
                    self.text(text);
                }
                refuse_calls([head.image.as_ref(), head.url.as_ref()])
            }
        }
    }

    fn form_part(&mut self, part: &FormPart) -> Result<(), LocatedError> {
        let field = match part {
            FormPart::Submit(submit) => {
                self.text(&submit.label);
                return Ok(());
            }
            FormPart::Field(field) => field,
        };
        self.text(&field.label);
        let [value, options, currency] = [&field.value, &field.options, &field.currency]
            .map(|attribute| attribute.as_ref().map(|attribute| &attribute.value));
        if let Some(Expression::Authored(name)) = options {
            self.enumeration(name, field.position);
        }
        refuse_calls([value, options, currency])
    }

    fn text(&mut self, text: &Expression) {
        match text {
            Expression::Interpolated(parts) => parts.iter().for_each(|part| self.text(part)),
            Expression::Call(call) => self.call(call),
            _ => {}
        }
    }

    fn call(&mut self, call: &Call) {
        let position = call.position();
        let (key, kind) = match call {
            Call::Translate { key, arguments, .. } => {
                let mut names: Vec<String> = arguments
                    .iter()
                    .map(|argument| argument.name.clone())
                    .collect();
                names.sort_unstable();
                for argument in arguments {
                    self.text(&argument.value);
                }
                (key, KeyKind::Message { arguments: names })
            }
            Call::Label { key, .. } => (key, KeyKind::Label),
            Call::Format { .. } => return,
        };
        self.uses.push(KeyUse {
            key: key.clone(),
            kind,
            line: position.line,
            column: position.column,
        });
    }

    fn enumeration(&mut self, name: &str, position: Position) {
        self.uses.push(KeyUse {
            key: name.to_owned(),
            kind: KeyKind::Enum,
            line: position.line,
            column: position.column,
        });
    }
}

fn refuse_calls<const N: usize>(expressions: [Option<&Expression>; N]) -> Result<(), LocatedError> {
    expressions.into_iter().flatten().try_for_each(refuse_call)
}

fn refuse_call(expression: &Expression) -> Result<(), LocatedError> {
    match expression {
        Expression::Call(call) => Err(LocatedError::new(
            call.position(),
            "`t(…)`, formats and labels write text; they cannot be a value, a URL or an id",
        )),
        Expression::Interpolated(parts) => parts.iter().try_for_each(refuse_call),
        _ => Ok(()),
    }
}
