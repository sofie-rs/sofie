use sofie::prelude::*;
use tracker_contract::area::Member;

use crate::queries;

pub fn routes(routes: &mut Routes<Member>) {
    routes.screen("/home", "home", queries::member_home);
}
