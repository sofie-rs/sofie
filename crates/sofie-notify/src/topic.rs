use serde::{Deserialize, Serialize};
use sofie_core::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Inbox,
    Mail,
    Push,
}

impl Channel {
    pub const ALL: [Self; 3] = [Self::Inbox, Self::Mail, Self::Push];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Mail => "mail",
            Self::Push => "push",
        }
    }

    pub(crate) fn parse(text: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|channel| channel.as_str() == text)
            .ok_or_else(|| Error::invalid("channel", "must be inbox, mail or push"))
    }
}

/// A kind of notification a module sends, named `<module>.<name>`; a principal turns
/// its channels on and off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Topic {
    pub(crate) name: &'static str,
    pub(crate) defaults: &'static [Channel],
    pub(crate) urgent: bool,
    pub(crate) digested: bool,
}

impl Topic {
    /// In the inbox only, until `channels` names more.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            defaults: &[Channel::Inbox],
            urgent: false,
            digested: true,
        }
    }

    /// The channels on for a principal who chose nothing.
    #[must_use]
    pub const fn channels(self, defaults: &'static [Channel]) -> Self {
        Self { defaults, ..self }
    }

    /// Sent through quiet hours and never held for a digest, such as a security notice.
    #[must_use]
    pub const fn urgent(self) -> Self {
        Self {
            urgent: true,
            digested: false,
            ..self
        }
    }

    /// Mailed at once even to a principal who reads mail as a digest.
    #[must_use]
    pub const fn never_digested(self) -> Self {
        Self {
            digested: false,
            ..self
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }
}
