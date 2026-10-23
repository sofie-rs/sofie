use crate::syntax::{Expression, Path, Root};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Field(String),
    Item,
}

/// What each open loop or table row name stands for, as steps from the data root.
#[derive(Default)]
pub(crate) struct Slots {
    /// `None` for an index, which holds no data.
    slots: Vec<Option<Vec<Step>>>,
}

impl Slots {
    pub(crate) fn push_items(&mut self, list: &Expression) {
        let items = self.items_of(list);
        self.slots.push(items);
    }

    pub(crate) fn push_dataless(&mut self) {
        self.slots.push(None);
    }

    #[cfg(feature = "gates")]
    pub(crate) fn depth(&self) -> usize {
        self.slots.len()
    }

    pub(crate) fn truncate(&mut self, depth: usize) {
        self.slots.truncate(depth);
    }

    pub(crate) fn steps_of(&self, path: &Path) -> Option<Vec<Step>> {
        let mut steps = match path.root {
            Root::Data => Vec::new(),
            Root::Slot(slot) => self.slots.get(slot)?.clone()?,
        };
        steps.extend(path.fields.iter().cloned().map(Step::Field));
        Some(steps)
    }

    pub(crate) fn items_of(&self, list: &Expression) -> Option<Vec<Step>> {
        let Expression::Path(path) = list else {
            return None;
        };
        let mut steps = self.steps_of(path)?;
        steps.push(Step::Item);
        Some(steps)
    }
}
