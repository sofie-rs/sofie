use sofie::http::{ActiveBlocks, Area, NeedsPermission, Permission, Routes, ShieldBlocks, Shipped};
use sofie::prelude::*;
use sofie::serde_json::{Value, json};

use super::Listed;

const READ: Permission = Permission::new("sofie_identity.shield.*.read");
const LIFT: Permission = Permission::new("sofie_identity.shield.*.delete");

pub(super) fn routes<A: Area>(routes: &mut Routes<A>) {
    let describe: sofie::http::Describe = |screens, key| screens.describe_form::<Lift>(key, "lift");
    routes
        .shipped(
            "/shield",
            "shield",
            (include_str!("shield.html"), Some(describe)),
            list,
        )
        .name("list_blocks")
        .permission(READ);
    routes
        .post("/shield", lift)
        .name("lift_block")
        .permission(LIFT)
        .then_screen("shield", "blocks");
}

async fn list(blocks: ActiveBlocks) -> Result<Listed> {
    let rows = blocks
        .list()
        .into_iter()
        .map(|block| {
            json!({
                "id": block.id,
                "caller": block.caller,
                "rule": block.rule,
                "grade": block.grade,
                "until": block.until.to_string(),
            })
        })
        .collect();
    Ok(Listed(Value::Array(rows)))
}

#[derive(Input)]
struct Lift {
    #[required]
    block: String,
}

async fn lift(blocks: ShieldBlocks, lift: Lift) -> Result<()> {
    blocks.lift(lift.block.trim()).await
}
