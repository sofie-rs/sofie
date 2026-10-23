pub use sofie_http::Entitlement;

pub const UNLIMITED: u64 = u64::MAX;

/// "At most N" of something the app counts itself, such as rows it keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit(&'static str);

impl Limit {
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.0
    }

    /// Held while the plan allows more than none, for a route, menu entry or screen.
    #[must_use]
    pub const fn entitlement(self) -> Entitlement {
        Entitlement::new(self.0)
    }
}

/// An amount used up within each period and given back at the next one's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quota {
    name: &'static str,
    period: Period,
}

impl Quota {
    #[must_use]
    pub const fn new(name: &'static str, period: Period) -> Self {
        Self { name, period }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    #[must_use]
    pub const fn period(self) -> Period {
        self.period
    }

    #[must_use]
    pub const fn entitlement(self) -> Entitlement {
        Entitlement::new(self.name)
    }
}

/// Counted from the subscription's start, so a month runs from its billing day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    Month,
    Year,
}

/// `<module>.<name>` in snake case, as a flag is named.
pub(crate) const fn is_entitlement_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let mut index = 0;
    let mut dots = 0;
    let mut part_start = true;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'.' {
            if part_start {
                return false;
            }
            dots += 1;
            part_start = true;
        } else {
            let lower = byte.is_ascii_lowercase();
            if part_start && !lower {
                return false;
            }
            if !(lower || byte.is_ascii_digit() || byte == b'_') {
                return false;
            }
            part_start = false;
        }
        index += 1;
    }
    dots == 1 && !part_start
}

#[cfg(test)]
mod tests {
    use super::is_entitlement_name;

    #[test]
    fn an_entitlement_is_named_module_dot_name() {
        assert!(is_entitlement_name("project.export"));
        assert!(is_entitlement_name("project.api_calls2"));
        for refused in [
            "export",
            "project.",
            ".export",
            "Project.export",
            "a.b.c",
            "project.ex-port",
        ] {
            assert!(!is_entitlement_name(refused), "{refused}");
        }
    }
}
