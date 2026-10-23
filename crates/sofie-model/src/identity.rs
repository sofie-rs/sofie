use crate::naming::IDENTITY_AREA;
use crate::package_routes::{DEFAULT_MOUNT, ListedRoute, PackageRoutes, ServedRoute, served_route};
use crate::sources::{ConditionalWiring, module_tree};
use crate::{Code, Identity, ModelError};

pub(crate) const IDENTITY_CRATE: &str = "sofie_identity";
/// The member routes, which the wiring mounts in every member area a contract defines.
const MEMBER_MOUNT: &str = DEFAULT_MOUNT;

impl crate::AreaDefinition {
    /// It implements sofie-identity's `MemberArea`, or `sofie gen` writes one for it.
    #[must_use]
    pub fn is_member_area(&self) -> bool {
        self.implements_member_area || self.needs_generated_member_area()
    }
}

impl<'app> PackageRoutes<'app> {
    /// The wiring mounts the member routes for every member area a contract defines; the
    /// area-generic ones are where the app's code mounts them.
    pub fn identity(&self) -> Result<Vec<ServedRoute<'app>>, ModelError> {
        let Some(identity) = &self.app.identity else {
            return Ok(Vec::new());
        };
        let mut served = self.member_routes(identity)?;
        let generic = identity
            .mounts
            .iter()
            .filter(|mount| mount.mount != MEMBER_MOUNT);
        served.extend(self.mounted(IDENTITY_CRATE, &identity.routes, generic));
        Ok(served)
    }

    fn member_routes(
        &self,
        identity: &'app Identity,
    ) -> Result<Vec<ServedRoute<'app>>, ModelError> {
        let routes: Vec<&ListedRoute> = identity
            .routes
            .iter()
            .filter(|route| route.mount == MEMBER_MOUNT)
            .collect();
        let mut served = Vec::new();
        if routes.is_empty() {
            return Ok(served);
        }
        for area in self.areas.iter().filter(|area| area.contract.is_some()) {
            let Some(definition) = area
                .definition
                .filter(|definition| definition.is_member_area())
            else {
                continue;
            };
            let (Some(prefix), Some(paths)) =
                (definition.prefix.as_deref(), definition.sign_in_paths()?)
            else {
                continue;
            };
            for route in &routes {
                if route.path.contains("{sign_up}") && paths.sign_up.is_none() {
                    continue;
                }
                let path = route
                    .path
                    .replace("{sign_in}", &paths.sign_in)
                    .replace("{sign_up}", paths.sign_up.as_deref().unwrap_or_default())
                    .replace("{prefix}", prefix);
                served.push(served_route(
                    IDENTITY_CRATE,
                    IDENTITY_AREA,
                    area.name,
                    path,
                    route,
                ));
            }
        }
        Ok(served)
    }
}

impl Identity {
    /// Read on demand: only `explain` needs sofie-identity's handlers.
    pub fn code(&self) -> Result<Code, ModelError> {
        Ok(module_tree(&self.directory, ConditionalWiring::Allowed)?
            .found
            .code)
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{self, TemporaryDirectory};

    #[test]
    fn signs_members_in_to_no_public_area_a_contract_defines() {
        let directory = TemporaryDirectory::new("sofie-model-identity-areas");
        let mut order = testing::module("order", directory.path("order"));
        testing::define_area(&mut order, "member");
        let mut docs = order.defined_areas[0].clone();
        docs.name = "docs".to_owned();
        docs.type_path[2] = "Docs".to_owned();
        docs.prefix = Some("/docs".to_owned());
        docs.principal = vec!["Visitor".to_owned()];
        docs.implements_member_area = false;
        order.defined_areas.push(docs);
        directory.write(
            "order/contract/src/area.rs",
            "impl Area for Member {\n    const SIGN_IN: &'static str = \"/sign-in\";\n}\n\
             impl Area for Docs {\n    const SIGN_IN: &'static str = \"/\";\n}\n",
        );
        let mut shop = testing::app(directory.root(), vec![order]);
        shop.identity = Some(crate::Identity {
            routes: vec![crate::ListedRoute {
                mount: "routes".to_owned(),
                method: "GET".to_owned(),
                path: "{sign_in}".to_owned(),
                handler: "sign_in_page".to_owned(),
                permission: None,
            }],
            ..crate::Identity::default()
        });

        let served = shop.package_routes().identity().unwrap();

        let areas: Vec<(&str, &str)> = served
            .iter()
            .map(|route| (route.name.as_str(), route.path.as_str()))
            .collect();
        assert_eq!(areas, [("sofie_identity.member.sign_in_page", "/sign-in")]);
    }
}
