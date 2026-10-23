use std::marker::PhantomData;

use sofie_core::{Error, Result, Uuid};
use sofie_db::{Id, Resolve};

use crate::{Area, AreaPrincipal, RequestContext};

/// The aggregate an area's principal signs in as.
pub type PrincipalRecord<A> = <<A as Area>::Principal as AreaPrincipal>::Record;

/// Resolves only in `A`, so a handler cannot ask for another area's principal.
pub struct Principal<A: Area> {
    id: Uuid,
    area: PhantomData<fn() -> A>,
}

impl<A: Area> Principal<A> {
    #[must_use]
    pub fn id(&self) -> Id<PrincipalRecord<A>> {
        Id::from_uuid(self.id)
    }
}

/// The area's guard required it before the handler ran.
impl<A: Area, M> Resolve<RequestContext<A, M>> for Principal<A> {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        let principal = context
            .session()
            .principal_in(A::NAME)
            .map(|id| Self {
                id,
                area: PhantomData,
            })
            .ok_or(Error::NotFound);
        std::future::ready(principal)
    }
}
