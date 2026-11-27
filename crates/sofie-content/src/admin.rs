use sofie::http::{
    Access, ActionRoute, Area, AreaPrincipal, ListedRoute, NeedsPermission, Permission, Resources,
};
use sofie::prelude::*;

use crate::Page;

/// `resource::<Page>()` adds the first six.
#[rustfmt::skip]
pub const ROUTES: &[ListedRoute] = &[
    ListedRoute::new("admin_routes", "GET", "/pages", "list_pages"),
    ListedRoute::new("admin_routes", "GET", "/pages/{page}", "show_page"),
    ListedRoute::new("admin_routes", "GET", "/pages/new", "new_page"),
    ListedRoute::new("admin_routes", "POST", "/pages/new", "create_page"),
    ListedRoute::new("admin_routes", "GET", "/pages/{page}/edit", "edit_page"),
    ListedRoute::new("admin_routes", "POST", "/pages/{page}/edit", "update_page"),
    ListedRoute::new("admin_routes", "POST", "/pages/{page}/publish", "publish"),
    ListedRoute::new("admin_routes", "POST", "/pages/{page}/withdraw", "withdraw"),
];

/// Pages listed, written and changed in `A` (best a `Permitted` area), published and withdrawn.
pub fn admin_routes<A: Area>(routes: &mut Routes<A>) {
    routes.resource::<Page>();
    permitted(
        routes
            .post("/pages/{page}/publish", publish)
            .name("publish"),
    );
    permitted(
        routes
            .post("/pages/{page}/withdraw", withdraw)
            .name("withdraw"),
    );
}

/// Publishing changes the page as editing does; only a `Permitted` area reads grants.
fn permitted<A: Area>(route: &mut ActionRoute<'_, A>) {
    if <A::Principal as AreaPrincipal>::ACCESS == Access::Permitted {
        route.permission(UPDATE);
    }
}

const UPDATE: Permission = Permission::new("sofie_content.page.*.update");

async fn publish(mut page: Page, clock: Clock) -> Result<Page> {
    page.publish(clock.now());
    Ok(page)
}

async fn withdraw(mut page: Page) -> Result<Page> {
    page.withdraw();
    Ok(page)
}
