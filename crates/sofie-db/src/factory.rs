use sofie_core::Result;

use crate::Input;

pub trait Create: Sized {
    type Input: Input;
    type Owner;

    fn create(input: Self::Input, owner: Self::Owner) -> Result<Self>;
}

pub trait Update {
    type Input: Input;

    fn update(&mut self, changes: Self::Input) -> Result<()>;
}
