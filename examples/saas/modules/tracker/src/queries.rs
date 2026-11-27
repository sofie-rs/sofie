use sofie::prelude::*;

#[derive(ReadModel)]
pub struct StaffHome {}

pub async fn staff_home(_read: Read) -> Result<StaffHome> {
    Ok(StaffHome {})
}

#[derive(ReadModel)]
pub struct MemberHome {}

pub async fn member_home(_read: Read) -> Result<MemberHome> {
    Ok(MemberHome {})
}
