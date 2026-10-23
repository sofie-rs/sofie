use crate::naming::RouteSite;
use crate::{App, Method, ServedRoute};

impl App {
    /// The POST route the router serves `post` with; a bound path part is written `{}`.
    #[must_use]
    pub fn posted_route(&self, post: &str) -> Option<RouteSite<'_>> {
        let posts: Vec<(RouteSite, String)> = self
            .route_sites()
            .into_iter()
            .filter(|site| site.route.method == Method::Post)
            .filter_map(|site| Some((site, site.full_path(self)?)))
            .collect();
        let paths: Vec<String> = posts.iter().map(|(_, path)| path.clone()).collect();
        posts.get(chosen(&paths, post)?).map(|(site, _)| *site)
    }
}

impl ServedRoute<'_> {
    /// Whether `post`, with a bound path part written `{}`, reaches this package route.
    #[must_use]
    pub fn takes_post(&self, post: &str) -> bool {
        self.route.method == "POST" && reaches(&self.path, post)
    }
}

/// Of the routes a post reaches, the one with a literal segment where another has a parameter.
fn chosen(paths: &[String], post: &str) -> Option<usize> {
    let literals = |path: &str| -> Vec<bool> {
        path.split('/')
            .map(|segment| !segment.starts_with('{'))
            .collect()
    };
    paths
        .iter()
        .enumerate()
        .filter(|(_, path)| reaches(path, post))
        .max_by(|(left_index, left), (right_index, right)| {
            literals(left)
                .cmp(&literals(right))
                .then(right_index.cmp(left_index))
        })
        .map(|(index, _)| index)
}

fn reaches(path: &str, post: &str) -> bool {
    let (routes, posts): (Vec<&str>, Vec<&str>) =
        (path.split('/').collect(), post.split('/').collect());
    routes.len() == posts.len()
        && routes
            .iter()
            .zip(&posts)
            .all(|(route, posted)| route.starts_with('{') || route == posted)
}

#[cfg(test)]
mod tests {
    use super::{chosen, reaches};
    use crate::{ListedRoute, ServedRoute};

    #[test]
    fn a_post_reaches_a_package_route_only_by_post() {
        let listed = |method: &str| ListedRoute {
            mount: "customer".to_owned(),
            method: method.to_owned(),
            path: "/sign-out".to_owned(),
            handler: "routes::sign_out".to_owned(),
            permission: None,
        };
        let served = |route| ServedRoute {
            area: "web".to_owned(),
            path: "/account/sign-out".to_owned(),
            name: "sofie_identity.customer.sign_out".to_owned(),
            route,
        };
        let (post, get) = (listed("POST"), listed("GET"));

        assert!(served(&post).takes_post("/account/sign-out"));
        assert!(!served(&get).takes_post("/account/sign-out"));
        assert!(!served(&post).takes_post("/account/sign-in"));
    }

    #[test]
    fn a_post_goes_to_the_route_the_router_prefers_a_literal_segment_over_a_parameter() {
        let paths = [
            "/orders/{order}".to_owned(),
            "/orders/new".to_owned(),
            "/orders/{order}/cancel".to_owned(),
        ];

        assert_eq!(chosen(&paths, "/orders/new"), Some(1));
        assert_eq!(chosen(&paths, "/orders/{}"), Some(0));
        assert_eq!(chosen(&paths, "/carts/new"), None);
    }

    #[test]
    fn a_post_reaches_the_route_its_literal_segments_name() {
        assert!(reaches("/account/orders", "/account/orders"));
        assert!(reaches(
            "/account/orders/{order}/cancel",
            "/account/orders/{}/cancel"
        ));
        assert!(!reaches("/account/orders", "/account/orders/{}"));
        assert!(!reaches(
            "/account/orders/{order}/cancel",
            "/account/orders/{}/ship"
        ));
    }
}
