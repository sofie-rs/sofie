#[cfg_attr(not(feature = "i18n"), path = "recorded.rs")]
mod call;
mod scope;
mod value;

use sofie_core::Shown;
use sofie_core::encoding::percent;

use self::scope::Scope;
#[cfg(any(feature = "islands", feature = "listing"))]
pub(crate) use self::value::each_field;
pub(crate) use self::value::{
    Resolved, field_of, fields_of, is_truthy, items_of, push_display, str_of, text_of,
    write_escaped, write_plain,
};
use crate::catalog::{self, Submitted};
use crate::escape::push_url;
use crate::language::Language;
use crate::syntax::{Conditional, Expression, LocatedError, Loop, Op};

pub(crate) fn render_document<'a>(
    ops: &'a [Op],
    data: Shown<'a>,
    capacity: usize,
    submitted: Option<&'a Submitted<'a>>,
    language: Language<'a>,
) -> Result<String, LocatedError> {
    let mut renderer = Renderer::new(data, capacity, Pass::Writing, language);
    renderer.submitted = submitted;
    renderer.render_ops(ops)?;
    Ok(renderer.out)
}

/// Conditions and loops are evaluated on the way, so a hidden branch is never reached by its id.
pub(crate) fn render_fragment<'a>(
    ops: &'a [Op],
    data: Shown<'a>,
    id: &'a str,
    capacity: usize,
    language: Language<'a>,
) -> Result<Option<String>, LocatedError> {
    if id.is_empty() {
        return Ok(None);
    }
    let mut renderer = Renderer::new(data, capacity, Pass::Seeking(id), language);
    renderer.render_ops(ops)?;
    Ok((renderer.pass == Pass::Done).then_some(renderer.out))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass<'a> {
    Seeking(&'a str),
    Writing,
    Done,
}

pub(crate) struct Renderer<'a> {
    scope: Scope<'a>,
    out: String,
    /// For values that must be checked before they are escaped.
    scratch: String,
    submitted: Option<&'a Submitted<'a>>,
    pass: Pass<'a>,
    pub(crate) language: Language<'a>,
}

impl<'a> Renderer<'a> {
    fn new(data: Shown<'a>, capacity: usize, pass: Pass<'a>, language: Language<'a>) -> Self {
        Self {
            scope: Scope::new(data),
            out: String::with_capacity(capacity),
            scratch: String::new(),
            submitted: None,
            pass,
            language,
        }
    }

    pub(crate) fn render_ops(&mut self, ops: &'a [Op]) -> Result<(), LocatedError> {
        for op in ops {
            if self.pass == Pass::Done {
                break;
            }
            self.render_op(op)?;
        }
        Ok(())
    }

    fn render_op(&mut self, op: &'a Op) -> Result<(), LocatedError> {
        let is_writing = self.pass == Pass::Writing;
        match op {
            Op::Markup(markup) if is_writing => self.out.push_str(markup),
            Op::Text(expression) if is_writing => self.write_text(expression),
            Op::Url(expression) if is_writing => self.write_url(expression),
            Op::Markup(_) | Op::Text(_) | Op::Url(_) => {}
            Op::Identified { id, body } => {
                let is_sought = self.reaches(id);
                self.render_ops(body)?;
                self.finish_if(is_sought);
            }
            Op::Component(component) => {
                let is_sought = component.id.as_ref().is_some_and(|id| self.reaches(id));
                if self.pass == Pass::Writing {
                    catalog::render(component, self)?;
                } else {
                    self.render_ops(component.children())?;
                }
                self.finish_if(is_sought);
            }
            Op::If(conditional) => self.render_ops(self.taken_branch(conditional))?,
            Op::Each(each) => self.render_each(each)?,
        }
        Ok(())
    }

    fn reaches(&mut self, id: &'a Expression) -> bool {
        let Pass::Seeking(sought) = self.pass else {
            return false;
        };
        let is_sought = self.plain_text(id) == sought;
        if is_sought {
            self.pass = Pass::Writing;
        }
        is_sought
    }

    fn finish_if(&mut self, is_sought: bool) {
        if is_sought {
            self.pass = Pass::Done;
        }
    }

    fn render_each(&mut self, each: &'a Loop) -> Result<(), LocatedError> {
        for (index, item) in items_of(self.scope.resolve(&each.list)).enumerate() {
            if self.pass == Pass::Done {
                break;
            }
            self.scope.bind(each, index, item);
            self.render_ops(&each.body)?;
            self.scope.unbind(each);
        }
        Ok(())
    }

    pub(crate) fn render_with_item(
        &mut self,
        item: Shown<'a>,
        ops: &'a [Op],
    ) -> Result<(), LocatedError> {
        self.scope.bind_item(item);
        let rendered = self.render_ops(ops);
        self.scope.unbind_item();
        rendered
    }

    fn taken_branch(&self, conditional: &'a Conditional) -> &'a [Op] {
        if is_truthy(self.resolve(&conditional.condition)) {
            &conditional.then_branch
        } else {
            &conditional.else_branch
        }
    }

    pub(crate) fn resolve(&self, expression: &'a Expression) -> Resolved<'a> {
        #[cfg(feature = "gates")]
        if let Expression::Gate(gate) = expression {
            return Some(Shown::Integer(i64::from(gate.holds(self.language.gates()))));
        }
        self.scope.resolve(expression)
    }

    pub(crate) fn write_money(&mut self, amount: sofie_values::Money) {
        self.language
            .write_money(&mut self.out, amount, &mut self.scratch);
    }

    pub(crate) fn write_instant(&mut self, instant: sofie_values::Instant) {
        crate::language::write_instant(self.language, &mut self.out, instant, &mut self.scratch);
    }

    pub(crate) fn output(&mut self) -> &mut String {
        &mut self.out
    }

    pub(crate) fn push_markup(&mut self, markup: &str) {
        self.out.push_str(markup);
    }

    /// Authored text is written as is; anything else is data, escaped.
    pub(crate) fn write_text(&mut self, expression: &'a Expression) {
        match expression {
            Expression::Authored(markup) => self.out.push_str(markup),
            Expression::Interpolated(parts) => {
                for part in parts {
                    self.write_text(part);
                }
            }
            Expression::Call(call) => call::write_call(self, call),
            _ => {
                let resolved = self.resolve(expression);
                write_escaped(&mut self.out, resolved);
            }
        }
    }

    pub(crate) fn write_id(&mut self, id: Option<&'a Expression>) {
        if let Some(id) = id {
            self.open_attribute("id");
            self.write_text(id);
            self.out.push('"');
        }
    }

    pub(crate) fn write_url_attribute(&mut self, name: &str, expression: &'a Expression) {
        self.open_attribute(name);
        self.write_url(expression);
        self.out.push('"');
    }

    pub(crate) fn write_url_attribute_with_item(
        &mut self,
        name: &str,
        expression: &'a Expression,
        item: Shown<'a>,
    ) {
        self.scope.bind_item(item);
        self.write_url_attribute(name, expression);
        self.scope.unbind_item();
    }

    pub(crate) fn write_url(&mut self, expression: &'a Expression) {
        self.scratch.clear();
        build_url(&self.scope, &mut self.scratch, expression);
        let is_authored = matches!(
            expression,
            Expression::Authored(_) | Expression::Interpolated(_)
        );
        push_url(&mut self.out, &self.scratch, is_authored);
    }

    pub(crate) fn submitted_to(&mut self, post: &'a Expression) -> Option<&'a Submitted<'a>> {
        let submitted = self.submitted?;
        self.scratch.clear();
        build_url(&self.scope, &mut self.scratch, post);
        (self.scratch == submitted.action).then_some(submitted)
    }

    fn open_attribute(&mut self, name: &str) {
        self.out.push(' ');
        self.out.push_str(name);
        self.out.push_str("=\"");
    }

    /// The text before escaping, as a check must read it.
    pub(crate) fn plain_text(&mut self, expression: &'a Expression) -> &str {
        self.scratch.clear();
        for part in parts_of(expression) {
            write_plain(&mut self.scratch, self.scope.resolve(part));
        }
        &self.scratch
    }
}

/// Each `{…}` part is percent-encoded, so data can fill a segment but never reshape the URL.
fn build_url<'a>(scope: &Scope<'a>, target: &mut String, expression: &'a Expression) {
    let Expression::Interpolated(parts) = expression else {
        write_plain(target, scope.resolve(expression));
        return;
    };
    for part in parts {
        match part {
            Expression::Authored(text) => target.push_str(text),
            _ => write_url_part(target, scope.resolve(part)),
        }
    }
}

fn write_url_part(target: &mut String, resolved: Resolved<'_>) {
    if let Some(text) = str_of(resolved) {
        percent::push_encoded(target, text, percent::is_unreserved);
        return;
    }
    match resolved {
        Some(Shown::Uuid(uuid)) => return uuid.push_to(target),
        // Digits and `-` need no percent-encoding.
        Some(Shown::Integer(_)) => return write_plain(target, resolved),
        _ => {}
    }
    let start = target.len();
    write_plain(target, resolved);
    let raw = target.split_off(start);
    percent::push_encoded(target, &raw, percent::is_unreserved);
}

pub(crate) fn parts_of(expression: &Expression) -> &[Expression] {
    match expression {
        Expression::Interpolated(parts) => parts,
        _ => std::slice::from_ref(expression),
    }
}
