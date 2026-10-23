use sofie::prelude::*;

use super::{NAME_TAKEN, Role};

pub async fn create_role(read: &Read, name: &str) -> Result<Role> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::invalid("name", "is required"));
    }
    let taken = Role::query()
        .where_(Role::NAME.eq(name.to_owned()))
        .exists(read)
        .await?;
    if taken {
        return Err(Error::invalid("name", NAME_TAKEN));
    }
    Ok(Role {
        meta: Meta::new()?,
        name: name.to_owned(),
    })
}

#[cfg(feature = "permitted-screens")]
#[derive(Input)]
pub struct RoleName {
    #[required]
    #[unique]
    name: String,
}

#[cfg(feature = "permitted-screens")]
impl sofie::db::Create for Role {
    type Input = RoleName;
    type Owner = ();

    fn create(input: RoleName, (): ()) -> Result<Self> {
        Ok(Role {
            meta: Meta::new()?,
            name: input.name.trim().to_owned(),
        })
    }
}

#[cfg(feature = "permitted-screens")]
impl sofie::db::Update for Role {
    type Input = RoleName;

    fn update(&mut self, input: RoleName) -> Result<()> {
        input.name.trim().clone_into(&mut self.name);
        Ok(())
    }
}

/// The role of that name, for a task or a seed naming one created before.
pub async fn role_named(read: &Read, name: &str) -> Result<super::RoleId> {
    Role::query()
        .where_(Role::NAME.eq(name.trim().to_owned()))
        .first(read)
        .await?
        .map(|role| role.id())
        .ok_or_else(|| Error::invalid("role", format!("`{name}` is not a role")))
}
