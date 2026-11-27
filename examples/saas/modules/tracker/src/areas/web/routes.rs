use sofie::prelude::*;

use crate::queries;

pub fn routes(routes: &mut Routes<Web>) {
    routes.screen("/pricing", "pricing", queries::list_plans);
}
