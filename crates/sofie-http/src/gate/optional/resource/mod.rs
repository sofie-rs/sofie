//! A module overrides a built-in screen with `screens/<name>.html`, such as `order_index.html`.

mod handlers;
#[cfg(feature = "history")]
mod history;
mod screens;

use http::Method;
use sofie_core::{Constraint, SqlType};
use sofie_db::{Aggregate, Create, Field, ReplyData, Resolve, Update};
use sofie_values::{Control, FormField, FormFields, Part};

pub use self::handlers::OwnerOf;
use self::handlers::{Edited, Owner, Posted};
use crate::handler::{Endpoint, Loader, endpoint, loader_endpoint};
use std::sync::Arc;

use crate::routes::{Refresh, RouteRules, ScreenView, View};
use crate::{Area, RequestContext, Routes, screen_key};

pub(crate) enum Builtin {
    Resource {
        description: Arc<Description>,
        kept: Arc<[Resource]>,
        screen: Resource,
        permission: Option<String>,
    },
    Shipped {
        source: &'static str,
        describe: Option<Describe>,
    },
}

pub type Describe = fn(&mut crate::Screens, &str) -> sofie_core::Result<()>;

/// Runs before the routes are checked against the screens.
pub(crate) fn install(
    screens: &mut crate::Screens,
    routes: &[crate::routes::Route],
) -> sofie_core::Result<()> {
    for route in routes {
        let Some((key, builtin)) = built_in(route) else {
            continue;
        };
        if !screens.contains(key) {
            screens.add_source(key, &builtin.source(&route.path))?;
        }
        if let Some(describe) = builtin.describe() {
            describe(screens, key)?;
        }
    }
    Ok(())
}

/// What `sofie scaffold screen` writes out for a module to override.
#[must_use]
pub fn built_in_screen_source(app: &crate::App, key: &str) -> Option<String> {
    app.routes.iter().find_map(|route| {
        let (candidate, builtin) = built_in(route)?;
        (candidate == key).then(|| builtin.source(&route.path))
    })
}

fn built_in(route: &crate::routes::Route) -> Option<(&str, &Builtin)> {
    let View::Screen(ScreenView {
        key,
        builtin: Some(builtin),
        ..
    }) = &route.view
    else {
        return None;
    };
    Some((key, builtin))
}

impl Builtin {
    fn source(&self, path: &str) -> String {
        match self {
            Self::Shipped { source, .. } => (*source).to_owned(),
            Self::Resource {
                description,
                kept,
                screen,
                permission,
            } => screens::Plan {
                description,
                at: list_path(path, *screen, description.name),
                kept,
                permission: permission.as_deref(),
            }
            .source(*screen),
        }
    }

    fn describe(&self) -> Option<Describe> {
        match self {
            Self::Shipped { describe, .. } => *describe,
            Self::Resource {
                description,
                screen,
                ..
            } => match screen {
                Resource::Create => Some(description.create),
                Resource::Edit => Some(description.edit),
                _ => None,
            },
        }
    }
}

pub trait Shipped<A: Area> {
    /// Rendered from `source` unless the module writes `screens/<name>.html`.
    fn shipped<Args: 'static, H>(
        &mut self,
        path: &str,
        name: &str,
        template: (&'static str, Option<Describe>),
        loader: H,
    ) -> crate::PageRoute<'_, A>
    where
        H: Loader<A, Args>;
}

impl<A: Area> Shipped<A> for Routes<A> {
    fn shipped<Args: 'static, H>(
        &mut self,
        path: &str,
        name: &str,
        (source, describe): (&'static str, Option<Describe>),
        loader: H,
    ) -> crate::PageRoute<'_, A>
    where
        H: Loader<A, Args>,
    {
        let view = View::Screen(ScreenView {
            key: screen_key(self.module, A::NAME, name),
            title: sofie_engine::humanize(name),
            builtin: Some(Arc::new(Builtin::Shipped { source, describe })),
        });
        crate::PageRoute::new(self.push(
            Method::GET,
            path,
            loader_endpoint(loader),
            view,
            RouteRules::default(),
        ))
    }
}

fn list_path(path: &str, screen: Resource, name: &str) -> String {
    let suffix = match screen {
        Resource::Show => format!("/{{{name}}}"),
        Resource::Create => "/new".to_owned(),
        Resource::Edit => format!("/{{{name}}}/edit"),
        Resource::Index | Resource::Delete => String::new(),
    };
    path.strip_suffix(&suffix).unwrap_or(path).to_owned()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Index,
    Show,
    Create,
    Edit,
    /// Offered only for an aggregate with `SoftDelete`.
    Delete,
}

pub struct ResourceRoutes<'r, A: Area> {
    routes: &'r mut Routes<A>,
    kept: Vec<Resource>,
    parts: Option<Parts>,
}

struct Parts {
    description: Description,
    list: Endpoint,
    show: Endpoint,
    blank: Endpoint,
    create: Endpoint,
    update: Endpoint,
    delete: Endpoint,
}

pub(crate) struct Description {
    #[cfg_attr(
        not(feature = "permissions"),
        expect(dead_code, reason = "only permissions are named by it")
    )]
    module: &'static str,
    name: &'static str,
    /// The edit form's, from the update input.
    edited: &'static [FormField],
    /// The record's own, which its list and show screens present.
    shown: &'static [ShownField],
    create: Describe,
    edit: Describe,
    sortable: Vec<&'static str>,
    filters: Vec<&'static str>,
}

pub trait Resources<A: Area> {
    /// Under `/<plural of its name>`; the owner of a new record is the principal.
    fn resource<R>(&mut self) -> ResourceRoutes<'_, A>
    where
        R: Aggregate + ReplyData + Create<Owner: OwnerOf<A>> + Update,
        R: Resolve<RequestContext<A>> + Send,
        <R as Create>::Input: FormFields + Send,
        <R as Update>::Input: FormFields + Send;
}

impl<A: Area> Resources<A> for Routes<A> {
    fn resource<R>(&mut self) -> ResourceRoutes<'_, A>
    where
        R: Aggregate + ReplyData + Create<Owner: OwnerOf<A>> + Update,
        R: Resolve<RequestContext<A>> + Send,
        <R as Create>::Input: FormFields + Send,
        <R as Update>::Input: FormFields + Send,
    {
        let created = <<R as Create>::Input as FormFields>::FIELDS;
        let edited = edit_fields(<<R as Update>::Input as FormFields>::FIELDS, R::FIELDS);
        let unique = unique_indexes(R::NAME, created);
        let shown = shown_fields(R::FIELDS, R::SENSITIVE, [created, edited]);
        let named = |field: &ShownField| field.name;
        let parts = Parts {
            description: Description {
                module: R::MODULE,
                name: R::NAME,
                edited,
                shown,
                create: |screens, key| screens.describe_form::<<R as Create>::Input>(key, "create"),
                edit: |screens, key| {
                    let fields = <<R as Update>::Input as FormFields>::FIELDS;
                    screens.describe_fields(key, "edit", edit_fields(fields, R::FIELDS))
                },
                sortable: shown
                    .iter()
                    .map(named)
                    .filter(|name| sofie_db::Sort::<R>::by_field(name, false).is_some())
                    .collect(),
                filters: shown
                    .iter()
                    .map(named)
                    .filter(|name| !sofie_db::filterable::<R>(name).is_empty())
                    .collect(),
            },
            list: loader_endpoint::<A, _, _>(handlers::list::<R>),
            show: loader_endpoint::<A, _, _>(
                move |record: handlers::Record<R>, read, parameters: handlers::Parameters| {
                    handlers::show::<R>(record, read, parameters, shown)
                },
            ),
            blank: loader_endpoint::<A, _, _>(handlers::blank),
            create: endpoint::<A, _, _>(
                move |posted: Posted<<R as Create>::Input>, owner: Owner<R::Owner>| {
                    handlers::create::<R>(posted, owner, unique)
                },
            ),
            update: endpoint::<A, _, _>(move |record: R, edited: Edited<<R as Update>::Input>| {
                handlers::update::<R>(record, edited, unique)
            }),
            delete: endpoint::<A, _, _>(handlers::delete::<R>),
        };
        let deletable = R::BEHAVIORS
            .iter()
            .any(|behavior| behavior.hides() == Some("deleted_at"));
        let mut kept = vec![
            Resource::Index,
            Resource::Show,
            Resource::Create,
            Resource::Edit,
        ];
        if deletable {
            kept.push(Resource::Delete);
        }
        ResourceRoutes {
            routes: self,
            kept,
            parts: Some(parts),
        }
    }
}

impl<A: Area> ResourceRoutes<'_, A> {
    pub fn only<const N: usize>(mut self, kept: [Resource; N]) {
        self.kept = kept.to_vec();
    }
}

impl<A: Area> Drop for ResourceRoutes<'_, A> {
    fn drop(&mut self) {
        if let Some(parts) = self.parts.take() {
            parts.add(self.routes, &self.kept);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShownField {
    pub(crate) name: &'static str,
    pub(crate) look: Look,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Look {
    Text,
    Money,
    Percentage,
    Parts(&'static [Part]),
    /// A value of several columns no input describes, shown as its parts' text.
    Joined,
}

/// What the record holds, not what its input takes: a state the record sets itself is shown, and
/// ids, parents, owners and sensitive values are not. An input field of the same name tells how
/// a value is shown. Leaked once per resource, while the app is built.
fn shown_fields(
    record: &'static [Field],
    sensitive: &[&str],
    inputs: [&'static [FormField]; 2],
) -> &'static [ShownField] {
    let shown: Vec<ShownField> = record
        .iter()
        .filter(|field| !sensitive.contains(&field.name) && !is_reference(field))
        .filter_map(|field| {
            let control = inputs
                .iter()
                .flat_map(|fields| fields.iter())
                .find(|input| input.name == field.name)
                .map(|input| input.input.control);
            let look = match control {
                Some(Control::Password | Control::File) => return None,
                Some(Control::Money) => Look::Money,
                Some(Control::Percentage) => Look::Percentage,
                Some(Control::Parts(parts)) => Look::Parts(parts),
                _ if is_money(field) => Look::Money,
                _ if field.columns.len() > 1 => Look::Joined,
                _ => Look::Text,
            };
            Some(ShownField {
                name: field.name,
                look,
            })
        })
        .collect();
    Box::leak(shown.into_boxed_slice())
}

/// An `Id` of this or another record, such as a parent or an owner.
fn is_reference(field: &Field) -> bool {
    matches!(field.columns, [column] if column.ty == SqlType::Uuid)
}

fn is_money(field: &Field) -> bool {
    matches!(field.columns, [amount, currency]
        if amount.suffix.is_empty() && amount.ty == SqlType::Int && currency.suffix == "_currency")
}

/// A sensitive field is edited blank and a blank one keeps what the record stores, so it is
/// required only on create. Leaked once per resource, while the app is built.
fn edit_fields(fields: &'static [FormField], record: &[Field]) -> &'static [FormField] {
    let kept_blank = |field: &FormField| {
        handlers::is_secret(field)
            && record
                .iter()
                .any(|stored| stored.name == field.name && (stored.handed_back)())
    };
    if !fields.iter().any(kept_blank) {
        return fields;
    }
    let edited: Vec<FormField> = fields
        .iter()
        .map(|field| {
            if !kept_blank(field) {
                return *field;
            }
            let constraints: Vec<Constraint> = field
                .constraints
                .iter()
                .filter(|constraint| **constraint != Constraint::Required)
                .copied()
                .collect();
            FormField {
                required: false,
                constraints: Box::leak(constraints.into_boxed_slice()),
                ..*field
            }
        })
        .collect();
    Box::leak(edited.into_boxed_slice())
}

/// Leaked once per resource, while the app is built.
fn unique_indexes(
    table: &str,
    fields: &'static [FormField],
) -> &'static [(&'static str, &'static str)] {
    let unique: Vec<(&'static str, &'static str)> = fields
        .iter()
        .filter(|field| field.constraints.contains(&Constraint::Unique))
        .map(|field| {
            let index: &'static str = Box::leak(format!("{table}_{}", field.name).into_boxed_str());
            (index, field.name)
        })
        .collect();
    Box::leak(unique.into_boxed_slice())
}

/// As `sofie gen` names a resource's handlers.
fn handler_name(name: &str, action: &str, method: &Method, listing: bool) -> String {
    let verb = match (action, *method == Method::GET) {
        ("read", _) if listing => return format!("list_{}", sofie_core::plural(name)),
        ("read", _) => "show",
        ("create", true) => "new",
        ("update", true) => "edit",
        ("update", false) => "update",
        _ => action,
    };
    format!("{verb}_{name}")
}

fn plural_title(name: &str) -> String {
    sofie_engine::humanize(&sofie_core::plural(name))
}

impl Parts {
    fn add<A: Area>(self, routes: &mut Routes<A>, kept: &[Resource]) {
        let name = self.description.name;
        let module = routes.module;
        let at = format!("/{}", sofie_core::plural(name).replace('_', "-"));
        let record = format!("{at}/{{{name}}}");
        let key = |screen: &str| screen_key(module, A::NAME, &format!("{name}_{screen}"));
        let permission = permission_prefix::<A>(&self.description);
        let description = Arc::new(self.description);
        let kept: Arc<[Resource]> = kept.into();
        let screen = |resource: Resource, name: &str, title: String| {
            View::Screen(ScreenView {
                key: key(name),
                title,
                builtin: Some(Arc::new(Builtin::Resource {
                    description: Arc::clone(&description),
                    kept: Arc::clone(&kept),
                    screen: resource,
                    permission: permission.clone(),
                })),
            })
        };
        let refresh = |resource: Resource, name: &str, fragment: String| {
            if kept.contains(&resource) {
                return View::Refresh(Refresh {
                    screen: key(name),
                    fragment,
                });
            }
            View::Json
        };
        let list = || refresh(Resource::Index, "index", screens::list_id(name));
        let mut add = |method: Method, path: String, endpoint: &Endpoint, view, action| {
            let rules = route_rules(permission.as_deref(), action);
            let handler = handler_name(name, action, &method, path == at);
            routes
                .push(method, &path, Arc::clone(endpoint), view, rules)
                .name_as(A::NAME, &handler);
        };
        for resource in kept.iter() {
            match resource {
                Resource::Index => {
                    let view = screen(Resource::Index, "index", plural_title(name));
                    add(Method::GET, at.clone(), &self.list, view, "read");
                }
                Resource::Show => {
                    let view = screen(Resource::Show, "show", sofie_engine::humanize(name));
                    add(Method::GET, record.clone(), &self.show, view, "read");
                }
                Resource::Create => {
                    let view = screen(Resource::Create, "create", screens::titled("New", name));
                    let path = format!("{at}/new");
                    add(Method::GET, path.clone(), &self.blank, view, "create");
                    add(Method::POST, path, &self.create, list(), "create");
                }
                Resource::Edit => {
                    let view = screen(Resource::Edit, "edit", screens::titled("Edit", name));
                    let shown = refresh(Resource::Show, "show", "record".to_owned());
                    let path = format!("{record}/edit");
                    add(Method::GET, path.clone(), &self.show, view, "update");
                    add(Method::POST, path, &self.update, shown, "update");
                }
                Resource::Delete => {
                    let path = format!("{record}/delete");
                    add(Method::POST, path, &self.delete, list(), "delete");
                }
            }
        }
    }
}

#[cfg_attr(
    not(feature = "permissions"),
    expect(
        clippy::extra_unused_type_parameters,
        reason = "only `Permitted` areas ask"
    )
)]
fn permission_prefix<A: Area>(description: &Description) -> Option<String> {
    #[cfg(feature = "permissions")]
    if <A::Principal as crate::AreaPrincipal>::ACCESS == crate::Access::Permitted {
        return Some(format!("{}.{}.*.", description.module, description.name));
    }
    let _ = description;
    None
}

/// Leaked once per route while the app is built.
fn route_rules(permission_prefix: Option<&str>, action: &str) -> RouteRules {
    let _ = (permission_prefix, action);
    RouteRules {
        #[cfg(feature = "permissions")]
        permission: permission_prefix.map(|prefix| {
            crate::Permission::new(Box::leak(format!("{prefix}{action}").into_boxed_str()))
        }),
        ..RouteRules::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{Constraint, Control, Field, FormField, Shipped, built_in, edit_fields};
    use crate::{Routes, Web};

    async fn nothing() -> sofie_core::Result<()> {
        Ok(())
    }

    #[test]
    fn a_built_in_screen_gives_its_source_under_its_key() {
        let mut routes = Routes::<Web>::new("shop");
        routes.shipped("/welcome", "welcome", ("<p>Hi</p>", None), nothing);
        routes.get("/plain", nothing);

        let route = &routes.added[0];
        let (key, builtin) = built_in(route).unwrap();

        assert_eq!(key, "shop/web/welcome");
        assert_eq!(builtin.source(&route.path), "<p>Hi</p>");
        assert!(builtin.describe().is_none());
        assert!(built_in(&routes.added[1]).is_none());
    }

    #[test]
    fn a_password_the_record_does_not_store_stays_required_on_edit() {
        const REQUIRED: &[Constraint] = &[Constraint::Required];
        let password = |name| FormField {
            name,
            input: sofie_values::Input::new(Control::Password),
            required: true,
            constraints: REQUIRED,
        };
        let fields: &'static [FormField] = Box::leak(Box::new([
            password("iban"),
            password("password"),
            password("api_key"),
        ]));
        let record = [
            Field::new("iban", &[], false),
            Field::new("password_hash", &[], false),
            Field::new("api_key", &[], false).handed_back_by(|| false),
        ];

        let edited = edit_fields(fields, &record);

        assert!(!edited[0].required && edited[0].constraints.is_empty());
        assert!(edited[1].required && edited[1].constraints == REQUIRED);
        assert!(edited[2].required && edited[2].constraints == REQUIRED);
    }
}
