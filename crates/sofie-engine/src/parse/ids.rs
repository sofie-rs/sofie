use crate::syntax::{Attribute, Expression, LocatedError};

/// The literal ids written so far: two, or one written once per item, would make a fragment,
/// an error slot or a target find the wrong element.
#[derive(Clone, Default)]
pub(super) struct Ids {
    claimed: Vec<String>,
    /// Inside `{#each}` or a `Table`'s rows.
    pub(super) is_repeated: bool,
}

impl Ids {
    pub(super) fn claim(&mut self, attributes: &[Attribute]) -> Result<(), LocatedError> {
        let Some(Attribute {
            value: Expression::Authored(id),
            position,
            ..
        }) = attributes.iter().find(|attribute| attribute.name == "id")
        else {
            return Ok(());
        };
        if self.is_repeated {
            bail!(
                *position,
                "the id `{id}` would repeat in every `{{#each}}` item or `Table` row; build it \
                 from the item, such as `id=\"row-{{row.id}}\"`"
            );
        }
        if self.claimed.contains(id) {
            bail!(
                *position,
                "the id `{id}` is given twice; ids are unique on a page"
            );
        }
        self.claimed.push(id.clone());
        Ok(())
    }

    /// An `{#if}`'s branches may each write the same id.
    pub(super) fn join(&mut self, other_branch: Self) {
        for id in other_branch.claimed {
            if !self.claimed.contains(&id) {
                self.claimed.push(id);
            }
        }
    }
}
