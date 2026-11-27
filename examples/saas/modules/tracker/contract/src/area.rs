use sofie::http::Area;
use sofie_identity::{Account, Permitted};
use sofie_tenancy::{Organization, OrganizationArea};

pub struct Staff;

impl Area for Staff {
    const NAME: &'static str = "staff";
    const PREFIX: &'static str = "/staff";
    const SIGN_IN: &'static str = "/staff/sign-in";
    type Principal = Permitted;
}

pub struct Member;

impl Area for Member {
    const NAME: &'static str = "member";
    const PREFIX: &'static str = "/member";
    const SIGN_IN: &'static str = "/member/sign-in";
    type Principal = Account;
}

impl sofie_identity::SignUp for Member {
    const PATH: &'static str = "/member/sign-up";
}

impl sofie_identity::MemberArea for Member {
    const HOME: &'static str = "/member/organizations";
    const SIGN_UP: Option<&'static str> = Some(<Self as sofie_identity::SignUp>::PATH);
}

pub struct Team;

impl Area for Team {
    const NAME: &'static str = "team";
    const PREFIX: &'static str = "/team";
    const SIGN_IN: &'static str = "/member/organizations";
    type Principal = Organization;
}

impl OrganizationArea for Team {
    const HOME: &'static str = "/team/tasks";
}
