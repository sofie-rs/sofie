use std::borrow::Cow;

use crate::catalog::Component;
use crate::escape::push_escaped;
use crate::syntax::{Expression, Op};

#[derive(Debug)]
pub(crate) struct Program<C = Component> {
    ops: Vec<Op<C>>,
    pub(crate) markup: String,
}

impl<C> Default for Program<C> {
    fn default() -> Self {
        Self {
            ops: Vec::new(),
            markup: String::new(),
        }
    }
}

impl<C> Program<C> {
    pub fn into_ops(mut self) -> Vec<Op<C>> {
        self.end_markup();
        self.ops
    }

    pub fn push(&mut self, op: Op<C>) {
        if let Op::Markup(markup) = &op {
            self.markup.push_str(markup);
            return;
        }
        self.end_markup();
        self.ops.push(op);
    }

    pub fn push_markup(&mut self, markup: &str) {
        self.markup.push_str(markup);
    }

    /// A value known when parsing is escaped now; any other when rendering.
    pub fn push_text(&mut self, expression: Expression) {
        match constant(&expression) {
            Some(text) => push_escaped(&mut self.markup, &text),
            None => self.push(Op::Text(expression)),
        }
    }

    fn end_markup(&mut self) {
        if !self.markup.is_empty() {
            self.ops.push(Op::Markup(std::mem::take(&mut self.markup)));
        }
    }
}

pub(crate) fn constant(expression: &Expression) -> Option<Cow<'_, str>> {
    match expression {
        Expression::Text(text) | Expression::Authored(text) => Some(Cow::Borrowed(text)),
        Expression::Integer(number) => Some(Cow::Owned(number.to_string())),
        _ => None,
    }
}
