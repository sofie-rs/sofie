use sofie_core::Result;
use sofie_db::{Aggregate, Resolve};

use crate::RequestContext;

/// Its principal resolves only in its own area.
pub trait Area: Send + Sync + 'static {
    /// The folder under a module's `src/areas/`, such as `web`.
    const NAME: &'static str;
    const PREFIX: &'static str;
    const SIGN_IN: &'static str;
    /// Accepts `Authorization: Bearer` beside the session cookie.
    #[cfg(feature = "tokens")]
    const TOKENS: bool = false;
    /// Read by `sofie gen` and the build script, which compile the area's screens for it.
    const CLIENT: Client = Client::Html;
    /// Fixes the area's access, so the two cannot disagree.
    type Principal: AreaPrincipal;
}

/// What an area's pages are: HTML over the wire, or the same templates compiled into a client
/// framework's components when the app is built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Client {
    #[default]
    Html,
    Svelte,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Each endpoint states its own limits.
    Public,
    /// Another owned record is not found, and a write to one is refused.
    Owned,
    #[cfg(feature = "permissions")]
    Permitted,
}

/// An aggregate is its own principal, owning its records; a marker such as sofie-identity's
/// `Permitted` names the aggregate that signs in under another access.
pub trait AreaPrincipal: Send + 'static {
    const ACCESS: Access;
    type Record: 'static;
}

impl<T: Aggregate> AreaPrincipal for T {
    const ACCESS: Access = Access::Owned;
    type Record = T;
}

pub struct Web;

impl Area for Web {
    const NAME: &'static str = "web";
    const PREFIX: &'static str = "";
    const SIGN_IN: &'static str = "/";
    type Principal = Visitor;
}

/// Another area cannot resolve it:
///
/// ```compile_fail
/// # use sofie_http::{Area, Routes, Visitor};
/// struct Console;
/// impl Area for Console {
///     const NAME: &'static str = "console";
///     const PREFIX: &'static str = "/console";
///     const SIGN_IN: &'static str = "/";
///     type Principal = Visitor;
/// }
/// async fn greet(_visitor: Visitor) -> sofie_core::Result<()> { Ok(()) }
/// fn routes(routes: &mut Routes<Console>) { routes.get("/", greet); }
/// ```
#[derive(Debug)]
pub struct Visitor(());

/// No one signs in as one, so its `Principal` is never found.
impl AreaPrincipal for Visitor {
    const ACCESS: Access = Access::Public;
    type Record = Self;
}

impl<M> Resolve<RequestContext<Web, M>> for Visitor {
    fn resolve(_context: &mut RequestContext<Web, M>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self(())))
    }
}
