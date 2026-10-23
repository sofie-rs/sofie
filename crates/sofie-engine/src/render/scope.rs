use sofie_core::Shown;

use super::value::{Resolved, field_of};
use crate::syntax::{Expression, Loop, Path, Root};

#[derive(Clone, Copy)]
enum Binding<'a> {
    Item(Shown<'a>),
    Index(usize),
}

pub(crate) struct Scope<'a> {
    data: Shown<'a>,
    bindings: Vec<Binding<'a>>,
}

impl<'a> Scope<'a> {
    pub(super) fn new(data: Shown<'a>) -> Self {
        Self {
            data,
            bindings: Vec::new(),
        }
    }

    pub(super) fn resolve(&self, expression: &'a Expression) -> Resolved<'a> {
        match expression {
            Expression::Path(path) => self.resolve_path(path),
            Expression::Text(text) | Expression::Authored(text) => Some(Shown::Text(text)),
            Expression::Integer(number) => Some(Shown::Integer(*number)),
            // The parser refuses these wherever a value is read.
            Expression::Interpolated(_) => None,
            Expression::Call(_) => None,
            #[cfg(feature = "gates")]
            Expression::Gate(_) => None,
        }
    }

    pub(super) fn bind(&mut self, each: &Loop, index: usize, item: Shown<'a>) {
        self.bind_item(item);
        if each.has_index {
            self.bindings.push(Binding::Index(index));
        }
    }

    pub(super) fn bind_item(&mut self, item: Shown<'a>) {
        self.bindings.push(Binding::Item(item));
    }

    pub(super) fn unbind(&mut self, each: &Loop) {
        let count = if each.has_index { 2 } else { 1 };
        self.bindings
            .truncate(self.bindings.len().saturating_sub(count));
    }

    pub(super) fn unbind_item(&mut self) {
        self.bindings.pop();
    }

    fn resolve_path(&self, path: &Path) -> Resolved<'a> {
        let mut current = match path.root {
            Root::Data => Some(self.data),
            Root::Slot(slot) => match self.bindings.get(slot) {
                Some(Binding::Item(item)) => Some(*item),
                Some(Binding::Index(index)) => i64::try_from(*index).ok().map(Shown::Integer),
                None => None,
            },
        };
        for field in &path.fields {
            current = field_of(current, field);
        }
        current
    }
}
