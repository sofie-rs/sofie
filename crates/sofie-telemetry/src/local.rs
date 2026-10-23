use crate::TraceParent;
use crate::line::PendingLine;
use crate::subscriber::Export;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Trace {
    own: TraceParent,
    local_parent: bool,
}

impl Trace {
    pub(crate) fn open(parent: Option<&Self>) -> Self {
        Self {
            own: parent.map_or_else(TraceParent::root, |parent| parent.own.child()),
            local_parent: parent.is_some(),
        }
    }

    /// The span must not have children yet.
    pub(crate) fn continue_from(&mut self, parent: TraceParent) {
        self.own = parent.child();
        self.local_parent = false;
    }

    #[cfg(feature = "development")]
    pub(crate) fn is_local_root(&self) -> bool {
        !self.local_parent
    }

    pub(crate) fn traceparent(&self) -> TraceParent {
        self.own
    }
}

pub(crate) struct Exporter;

/// Without `otlp` nothing is exported, so no value exists.
pub(crate) enum LogExport {}

impl LogExport {
    pub(crate) fn send(&mut self, _: PendingLine) {
        match *self {}
    }

    pub(crate) fn take_dropped(&mut self) -> u64 {
        match *self {}
    }
}

impl Export for Exporter {}
