#[cfg(feature = "analysis")]
use crate::analysis::Recorder;
use crate::syntax::{Attribute, Expression, Position};

/// A name's index is the binding slot the renderer fills.
#[derive(Default)]
pub(super) struct Scope {
    names: Vec<String>,
    #[cfg(feature = "analysis")]
    pub(super) recorder: Recorder,
}

#[cfg_attr(
    not(feature = "analysis"),
    expect(unused_variables, clippy::unused_self)
)]
impl Scope {
    pub(super) fn names(&self) -> &[String] {
        &self.names
    }

    pub(super) fn depth(&self) -> usize {
        self.names.len()
    }

    pub(super) fn enter_loop(&mut self, item: String, index: Option<String>, list: &Expression) {
        self.names.push(item);
        #[cfg(feature = "analysis")]
        self.recorder.push_items(list);
        if let Some(index) = index {
            self.names.push(index);
            #[cfg(feature = "analysis")]
            self.recorder.push_index();
        }
    }

    pub(super) fn enter_rows(&mut self, name: String, props: &[Attribute]) {
        self.names.push(name);
        #[cfg(feature = "analysis")]
        self.recorder.push_rows(props);
    }

    pub(super) fn leave(&mut self, depth: usize) {
        self.names.truncate(depth);
        #[cfg(feature = "analysis")]
        self.recorder.truncate(depth);
    }

    #[cfg(feature = "islands")]
    pub(super) fn mark_island(&mut self, props: &[Attribute]) {
        #[cfg(feature = "analysis")]
        {
            let attribute = |name: &str| props.iter().find(|attribute| attribute.name == name);
            if let (Some(name), Some(given)) = (attribute("name"), attribute("props"))
                && let Expression::Authored(tag) = &name.value
            {
                self.recorder.mark_island(tag, given.position);
            }
        }
    }

    pub(super) fn record_action(&mut self, props: &[Attribute], position: Position) {
        #[cfg(feature = "analysis")]
        self.recorder.record_action(props, position);
    }

    pub(super) fn record_table_columns(&mut self, props: &[Attribute]) {
        #[cfg(feature = "analysis")]
        self.recorder.record_table_columns(props);
    }

    pub(super) fn record_table_link(&mut self, props: &[Attribute], position: Position) {
        #[cfg(feature = "analysis")]
        self.recorder.record_table_link(props, position);
    }

    pub(super) fn record(&mut self, expression: &Expression, position: Position) {
        #[cfg(feature = "analysis")]
        self.recorder.record(expression, position);
    }
}
