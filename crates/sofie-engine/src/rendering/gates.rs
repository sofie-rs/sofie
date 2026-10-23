//! A part the viewer may not see is never rendered.

pub trait Gates {
    fn can(&self, permission: &str) -> bool;

    fn enabled(&self, flag: &str) -> bool;

    /// What the viewer's plan holds: a switch, or a limit or quota above zero.
    fn entitled(&self, entitlement: &str) -> bool {
        let _ = entitlement;
        false
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct GatesRef<'a>(pub(crate) Option<&'a dyn Gates>);

impl std::fmt::Debug for GatesRef<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(if self.0.is_some() { "gated" } else { "ungated" })
    }
}

impl<'a> crate::language::Language<'a> {
    pub(crate) fn gates(self) -> Option<&'a dyn Gates> {
        self.gates.0
    }

    pub(crate) fn with_gates(mut self, gates: Option<&'a dyn Gates>) -> Self {
        self.gates = GatesRef(gates);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Gate {
    pub(crate) asks: Asks,
    pub(crate) name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asks {
    Permission,
    Flag,
    Entitlement,
}

impl Asks {
    /// As a client's view and runtime name the kind of gate.
    #[cfg(feature = "clients")]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Flag => "flag",
            Self::Entitlement => "entitlement",
        }
    }
}

impl Gate {
    /// Without gates, as when rendering for no request, nothing gated is shown.
    pub(crate) fn holds(&self, gates: Option<&dyn Gates>) -> bool {
        gates.is_some_and(|gates| match self.asks {
            Asks::Permission => gates.can(&self.name),
            Asks::Flag => gates.enabled(&self.name),
            Asks::Entitlement => gates.entitled(&self.name),
        })
    }
}
