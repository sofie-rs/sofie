/// A row of a package's `ROUTES` table, named `<crate>.<area>.<handler>` once mounted;
/// `mount` is the route function adding it. `sofie routes` reads the tables without
/// compiling, so every row stays a call with literal arguments.
#[derive(Clone, Copy, Debug)]
pub struct ListedRoute {
    pub mount: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub handler: &'static str,
    pub permission: Option<&'static str>,
}

impl ListedRoute {
    #[must_use]
    pub const fn new(
        mount: &'static str,
        method: &'static str,
        path: &'static str,
        handler: &'static str,
    ) -> Self {
        Self {
            mount,
            method,
            path,
            handler,
            permission: None,
        }
    }
}
