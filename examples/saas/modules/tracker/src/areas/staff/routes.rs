use sofie::prelude::*;
use tracker_contract::area::Staff;

use crate::queries;

pub fn routes(routes: &mut Routes<Staff>) {
    routes.screen("/home", "home", queries::staff_home);
}
