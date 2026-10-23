use proc_macro2::TokenStream;
use quote::quote;
use sofie_model::{App, FeatureDeclaration, Module};

use super::{in_module, rooted, route_key};

pub(super) struct Gating<'app> {
    app: &'app App,
    flags: bool,
    permissions: bool,
    /// sofie-billing's plans, installed as `billing` before the app is built.
    entitlements: bool,
}

impl<'app> Gating<'app> {
    pub(super) fn of(app: &'app App) -> Self {
        Self {
            app,
            flags: app.has_feature_flags(),
            permissions: app.has_permitted_areas(),
            entitlements: app.modules.iter().any(|module| module.plans.is_some()),
        }
    }

    pub(super) fn imports(&self) -> (Vec<TokenStream>, Option<TokenStream>) {
        let mut http = Vec::new();
        if self.has_gates() {
            http.push(quote!(Gates));
        }
        if self.flags {
            http.push(quote!(RouteFeature));
        }
        if self.permissions {
            http.extend([quote!(Permission), quote!(RoutePermission)]);
        }
        let flags = self.flags.then(|| {
            quote!(
                use sofie::flags::Flags;
            )
        });
        (http, flags)
    }

    pub(super) fn flags_parameter(&self, borrowed: bool) -> Option<TokenStream> {
        let reference = borrowed.then(|| quote!(&));
        self.flags.then(|| quote!(, flags: #reference Flags))
    }

    pub(super) fn app_call(&self) -> Option<TokenStream> {
        let features = self
            .flags
            .then(|| quote!(.features(flags).route_features(ROUTE_FEATURES)));
        let permissions = self
            .permissions
            .then(|| quote!(.route_permissions(ROUTE_PERMISSIONS)));
        let entitlements = self
            .entitlements
            .then(|| quote!(.entitlements(billing.clone())));
        self.has_gates()
            .then(|| quote!(.gates(Gates::default() #features #permissions #entitlements)))
    }

    fn has_gates(&self) -> bool {
        self.flags || self.permissions || self.entitlements
    }

    /// A permitted principal changes only the settings the modules declare, held by the
    /// app's `Flags`.
    pub(super) fn declared_settings(&self) -> Option<TokenStream> {
        let declares = |module: &Module| {
            module
                .manifest
                .as_ref()
                .is_some_and(|manifest| !manifest.settings.is_empty())
        };
        (self.flags && self.app.modules.iter().any(declares))
            .then(|| quote!(sofie::declare_settings(&flags, MODULES)?;))
    }

    pub(super) fn tables(&self) -> TokenStream {
        let sites = self.app.route_sites();
        let features = self.flags.then(|| {
            let entries = sites.iter().filter_map(|site| {
                let declaration = site.module.code.features.gate_on(&site.route.handler)?;
                let (method, path) = route_key(self.app, site);
                let feature = feature_path(site.module, declaration);
                Some(quote!(RouteFeature { method: #method, path: #path, feature: #feature }))
            });
            quote!(const ROUTE_FEATURES: &[RouteFeature] = &[#(#entries),*];)
        });
        let permissions = self.permissions.then(|| {
            let entries = sites.iter().filter_map(|site| {
                let permission = site.permission(self.app)?;
                let (method, path) = route_key(self.app, site);
                Some(quote! {
                    RoutePermission { method: #method, path: #path, permission: Permission::new(#permission) }
                })
            });
            quote!(const ROUTE_PERMISSIONS: &[RoutePermission] = &[#(#entries),*];)
        });
        quote!(#features #permissions)
    }
}

/// Named from the wiring that registers it: see `Module::job_side`.
pub(super) fn job_function(module: &Module, function: &[String]) -> TokenStream {
    let handler = rooted(module, module.job_side(function), function);
    match module.code.features.gate_on(function) {
        Some(declaration) => {
            let feature = feature_path(module, declaration);
            quote!(sofie::flags::gated(flags, #feature, #handler))
        }
        None => handler,
    }
}

fn feature_path(module: &Module, declaration: &FeatureDeclaration) -> TokenStream {
    in_module(module, &declaration.constant)
}
