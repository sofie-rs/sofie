//! Each `sofie new` generator described once, for the command line and the MCP tools alike.

use std::collections::BTreeMap;

use super::{AccountScreen, AreaAccess, NewCode, NewMenuEntry, NewRoute};

pub struct NewCommand {
    /// As in `sofie new <name>`.
    pub name: &'static str,
    pub tool: &'static str,
    pub description: &'static str,
    pub arguments: &'static [NewArgument],
    build: fn(&Values) -> Result<NewCode, String>,
}

pub struct NewArgument {
    pub name: &'static str,
    /// As the usage line shows it.
    pub shown: &'static str,
    pub description: &'static str,
    /// Written `--<name> VALUE` on the command line, after the others.
    pub optional: bool,
}

impl NewArgument {
    /// The option's command-line flag, `read_model` written `read-model`.
    fn flag(&self) -> String {
        self.name.replace('_', "-")
    }
}

struct Values(BTreeMap<&'static str, String>);

impl Values {
    fn text(&self, name: &str) -> String {
        self.0.get(name).cloned().unwrap_or_default()
    }

    fn optional(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }
}

impl NewCommand {
    /// The code to write and the arguments after this command's own, or what is wrong with them.
    pub fn parse<'arguments>(
        &self,
        mut arguments: &'arguments [String],
    ) -> Result<(NewCode, &'arguments [String]), String> {
        let mut values = BTreeMap::new();
        for argument in self.arguments.iter().filter(|argument| !argument.optional) {
            let (value, rest) = arguments.split_first().ok_or_else(|| {
                format!("`{}` is missing; usage: {}", argument.shown, self.usage())
            })?;
            values.insert(argument.name, value.clone());
            arguments = rest;
        }
        while let [flag, value, rest @ ..] = arguments
            && let Some(argument) = self.arguments.iter().find(|argument| {
                argument.optional && flag.strip_prefix("--") == Some(argument.flag().as_str())
            })
        {
            values.insert(argument.name, value.clone());
            arguments = rest;
        }
        let code = (self.build)(&Values(values))?;
        Ok((code, arguments))
    }

    pub fn build(&self, value: impl Fn(&str) -> Option<String>) -> Result<NewCode, String> {
        let values = self
            .arguments
            .iter()
            .filter_map(|argument| Some((argument.name, value(argument.name)?)))
            .collect();
        (self.build)(&Values(values))
    }

    #[must_use]
    pub fn usage(&self) -> String {
        self.invocation() + " [ROOT]"
    }

    /// Run from the app root.
    pub(crate) fn invocation(&self) -> String {
        let mut line = format!("sofie new {}", self.name);
        for argument in self.arguments {
            if argument.optional {
                line = line + " [--" + &argument.flag() + " " + argument.shown + "]";
            } else {
                line.push(' ');
                line.push_str(argument.shown);
            }
        }
        line
    }
}

const fn required(
    name: &'static str,
    shown: &'static str,
    description: &'static str,
) -> NewArgument {
    NewArgument {
        name,
        shown,
        description,
        optional: false,
    }
}

const fn optional(
    name: &'static str,
    shown: &'static str,
    description: &'static str,
) -> NewArgument {
    NewArgument {
        name,
        shown,
        description,
        optional: true,
    }
}

const MODULE: NewArgument = required(
    "module",
    "MODULE",
    "The module's name, as in `modules/<module>/`.",
);

const HANDLER: NewArgument = required("handler", "handler", "The function's name in snake_case.");

pub const NEW_COMMANDS: &[NewCommand] = &[
    NewCommand {
        name: "migration",
        tool: "new_migration",
        description: "`sofie new migration`: the module's next empty `migrations/NNNN_<name>.sql`; \
                      then `sofie gen` and `sofie check`.",
        arguments: &[
            MODULE,
            required("name", "NAME", "The migration's name in snake_case."),
        ],
        build: |values| {
            Ok(NewCode::Migration {
                module: values.text("module"),
                name: values.text("name"),
            })
        },
    },
    NewCommand {
        name: "aggregate",
        tool: "new_aggregate",
        description: "`sofie new aggregate`: `src/<name>/mod.rs` with `#[derive(Aggregate)]`, its \
                      `mod` line and re-export in `lib.rs`, and a migration creating its table; \
                      then `sofie gen` and `sofie check`, which reports it unused until code names it.",
        arguments: &[
            MODULE,
            required("name", "Name", "The aggregate's type name, as `Invoice`."),
        ],
        build: |values| {
            Ok(NewCode::Aggregate {
                module: values.text("module"),
                name: values.text("name"),
            })
        },
    },
    NewCommand {
        name: "event",
        tool: "new_event",
        description: "`sofie new event`: an event in the module's `contract/src/events.rs`, \
                      re-exported by the contract; then `sofie gen` and `sofie check`.",
        arguments: &[
            MODULE,
            required("name", "Name", "The event's type name, as `InvoicePaid`."),
        ],
        build: |values| {
            Ok(NewCode::Event {
                module: values.text("module"),
                name: values.text("name"),
            })
        },
    },
    NewCommand {
        name: "listener",
        tool: "new_listener",
        description: "`sofie new listener`: an `#[on]` function in the module's `src/listeners.rs`, \
                      importing the event from the contract declaring it; then `sofie gen` and \
                      `sofie check`.",
        arguments: &[
            MODULE,
            required("event", "Event", "The event's type name."),
            HANDLER,
        ],
        build: |values| {
            Ok(NewCode::Listener {
                module: values.text("module"),
                event: values.text("event"),
                handler: values.text("handler"),
            })
        },
    },
    NewCommand {
        name: "task",
        tool: "new_task",
        description: "`sofie new task`: a `#[task]` function in the module's `src/tasks.rs`, the \
                      `tasks` feature on its `sofie` dependency and, for the app's first task, \
                      `main` running the wiring's `tasks`; then `sofie gen` and `sofie check`.",
        arguments: &[MODULE, HANDLER],
        build: |values| {
            Ok(NewCode::Task {
                module: values.text("module"),
                name: values.text("handler"),
            })
        },
    },
    NewCommand {
        name: "search",
        tool: "new_search",
        description: "`sofie new search`: full-text search over an aggregate's fields whose values \
                      are searched by their words or whole: `Searched` among its behaviors, a \
                      migration adding `search_text`, one building its index concurrently, and the \
                      `search` feature on the module's `sofie` dependency. Query with `Aggregate::matching(\"words\")`; then `sofie gen` \
                      and `sofie check`.",
        arguments: &[
            MODULE,
            required(
                "aggregate",
                "Aggregate",
                "The aggregate's type name, as `Product`.",
            ),
        ],
        build: |values| {
            Ok(NewCode::Search {
                module: values.text("module"),
                aggregate: values.text("aggregate"),
            })
        },
    },
    NewCommand {
        name: "plan",
        tool: "new_plan",
        description: "`sofie new plan`: a `Plan` constant in the module's `src/plans.rs`, listed in \
                      its `PLANS` (the first plan is the default every owner without a \
                      subscription holds), and the `billing` feature on its `sofie` dependency; \
                      one module per app declares plans. Then grant entitlements, limits and \
                      quotas on it, and run `sofie gen` and `sofie check`.",
        arguments: &[
            MODULE,
            required("name", "name", "The plan's name in snake case, as `pro`."),
        ],
        build: |values| {
            Ok(NewCode::Plan {
                module: values.text("module"),
                name: values.text("name"),
            })
        },
    },
    NewCommand {
        name: "mail",
        tool: "new_mail",
        description: "`sofie new mail`: a template in the module's `src/mails/<name>.html` and a \
                      function in `src/mails.rs` sending it, with the `mail` feature on its \
                      `sofie` dependency. With `list`, bulk mail of that list: one recipient, a \
                      one-click unsubscribe link, and `mail-web`, whose \
                      `sofie::mail::web::routes` a `Web` area mounts. Then `sofie gen` and \
                      `sofie check`.",
        arguments: &[
            MODULE,
            required(
                "name",
                "name",
                "The mail's name in snake case, as `weekly_news`.",
            ),
            optional(
                "list",
                "LIST",
                "For bulk mail: the list it belongs to, as `news`.",
            ),
        ],
        build: |values| {
            Ok(NewCode::Mail {
                module: values.text("module"),
                name: values.text("name"),
                list: values.optional("list"),
            })
        },
    },
    NewCommand {
        name: "area",
        tool: "new_area",
        description: "`sofie new area`: an area in the module's `contract/src/area.rs` under the \
                      prefix `/<name>`, whose principal fixes its access: `public` (a `Visitor`), \
                      `owned` (a sofie-identity `Account`, on its own records) or `permitted` \
                      (sofie-identity's `Permitted`, each route with its permission). Its first \
                      page is a `get /home` with a screen in `src/areas/<name>/`, where members \
                      land after signing in. An area members sign in to makes the contract and \
                      the app depend on sofie-identity, whose module and sign-in routes the \
                      wiring then adds. Then `sofie gen` and `sofie check`.",
        arguments: &[
            MODULE,
            required(
                "name",
                "NAME",
                "The area's name in snake case, as `console`.",
            ),
            required("access", "ACCESS", "`public`, `owned` or `permitted`."),
        ],
        build: |values| {
            let access = AreaAccess::parse(&values.text("access")).ok_or_else(|| {
                let names = AreaAccess::ALL.map(AreaAccess::name);
                format!("`access` is one of {}", names.join(", "))
            })?;
            Ok(NewCode::Area {
                module: values.text("module"),
                name: values.text("name"),
                access,
            })
        },
    },
    NewCommand {
        name: "account-screen",
        tool: "new_account_screen",
        description: "`sofie new account-screen`: mounts one of sofie-identity's screens of a \
                      member's own account (`sessions`, `two-factor`, `passkeys`, `connections`, \
                      `account-data`) for a member area in the app's `app`, and turns its feature \
                      on in the app's `sofie-identity` dependency. `connections` also needs \
                      `sofie_identity::oauth::install` at boot. Then `sofie gen` and `sofie check`.",
        arguments: &[
            required(
                "area",
                "AREA",
                "The member area the screen joins, as `member`.",
            ),
            required(
                "screen",
                "SCREEN",
                "`sessions`, `two-factor`, `passkeys`, `connections` or `account-data`.",
            ),
        ],
        build: |values| {
            let screen = AccountScreen::parse(&values.text("screen")).ok_or_else(|| {
                let names = AccountScreen::ALL.map(AccountScreen::name);
                format!("`screen` is one of {}", names.join(", "))
            })?;
            Ok(NewCode::AccountScreen {
                area: values.text("area"),
                screen,
            })
        },
    },
    NewCommand {
        name: "impersonation",
        tool: "new_impersonation",
        description: "`sofie new impersonation`: lets a principal of a `Permitted` area holding \
                      `sofie_identity.impersonation.*.create` sign in as a member of a member \
                      area, each time recorded with its reason, ended after half an hour or by \
                      `/impersonation/stop`: mounts both halves in the app's `app` and turns \
                      sofie-identity's `impersonation` on. Then `sofie gen` and `sofie check`.",
        arguments: &[
            required(
                "permitted",
                "PERMITTED_AREA",
                "The `Permitted` area impersonating, as `console`.",
            ),
            required(
                "member",
                "MEMBER_AREA",
                "The member area impersonated, as `member`.",
            ),
        ],
        build: |values| {
            Ok(NewCode::Impersonation {
                permitted: values.text("permitted"),
                member: values.text("member"),
            })
        },
    },
    NewCommand {
        name: "tenancy",
        tool: "new_tenancy",
        description: "`sofie new tenancy`: organizations that own records, members with roles and \
                      invitations by email (sofie-tenancy). The organization area's principal \
                      is `sofie_tenancy::OrganizationId` and it implements \
                      `sofie_tenancy::OrganizationArea`; the member area lists, founds and enters \
                      organizations and accepts invitations. Mounts both and makes the app depend \
                      on sofie-tenancy, whose migrations the wiring then adds. Then `sofie gen` and \
                      `sofie check`.",
        arguments: &[
            required(
                "organization",
                "ORGANIZATION_AREA",
                "The area acting for an organization, as `workspace`.",
            ),
            required(
                "member",
                "MEMBER_AREA",
                "The member area of accounts, as `member`.",
            ),
        ],
        build: |values| {
            Ok(NewCode::Tenancy {
                organization: values.text("organization"),
                member: values.text("member"),
            })
        },
    },
    NewCommand {
        name: "route",
        tool: "new_route",
        description: "`sofie new route`: a handler stub in `actions` (writes) or `queries` (`get`), \
                      the route in `src/areas/<area>/routes.rs` and, with `screen`, the screen \
                      file. The path is relative to the area's prefix: `/orders/{order}` in an \
                      area prefixed `/account` serves `/account/orders/{order}`. A write whose \
                      path names an aggregate (`{invoice}`) takes and returns it, a `get` takes a \
                      read model of it loaded from the path (named after the handler, or \
                      `read_model`), with the aggregate's fields (or `fields`), and its screen \
                      shows them, an instant through `date(…)`, under `title` (its heading and \
                      tab) and a link back to \
                      the area's page listing the aggregate (or `back`), each choice named in the \
                      output. By default the read model leaves out the owner, another record's id \
                      and a secret: a field marked `#[sensitive]` or typed `Secret<…>` or a value \
                      the app marks `#[sensitive]`. Personal data such as an email stays, as the \
                      page shows the viewer's own record, unless `fields` leaves it out. The page \
                      shows the record's id only when `fields` names `id`. Then `sofie gen` and \
                      `sofie check`.",
        arguments: &[
            MODULE,
            required("area", "AREA", "An area the app knows, as `web`."),
            required(
                "method",
                "METHOD",
                "`get`, `post`, `put`, `patch` or `delete`, in either case.",
            ),
            required(
                "path",
                "PATH",
                "The path relative to the area's prefix, starting with `/`.",
            ),
            HANDLER,
            optional(
                "screen",
                "NAME",
                "For a `get`: the screen it shows, written to `screens/<screen>.html`.",
            ),
            optional(
                "fields",
                "FIELDS",
                "For a `get` naming an aggregate: the fields its read model reads, comma \
                 separated; without it, all but the owner, another record's id and a secret \
                 (marked `#[sensitive]`, or typed `Secret<…>` or a value the app marks \
                 `#[sensitive]`), each named in the output. Personal data such as an email \
                 stays unless `fields` leaves it out.",
            ),
            optional(
                "read_model",
                "Name",
                "For a `get` with a screen or naming an aggregate: the read model's type name, \
                 as `InvoiceDetail`; named after the handler without it.",
            ),
            optional(
                "title",
                "TITLE",
                "For a `get` with a screen: the page's heading and its browser tab's title, as \
                 `Your order`; the screen's name humanized without it.",
            ),
            optional(
                "back",
                "HANDLER",
                "For a `get` with a screen: the handler of the area's `get` page without \
                 parameters it links back to, or `none`; without it, a page naming an aggregate \
                 links to the area's page listing it (its read model reads the aggregate, or its \
                 path is the record path's plural, as `/orders` for `/orders/{order}`).",
            ),
        ],
        build: |values| {
            Ok(NewCode::Route(NewRoute {
                module: values.text("module"),
                area: values.text("area"),
                method: values.text("method"),
                path: values.text("path"),
                handler: values.text("handler"),
                screen: values.optional("screen"),
                fields: values.optional("fields").map(|fields| {
                    fields
                        .split(',')
                        .map(|field| field.trim().to_owned())
                        .collect()
                }),
                read_model: values.optional("read_model"),
                title: values.optional("title"),
                back: values.optional("back"),
            }))
        },
    },
    NewCommand {
        name: "menu",
        tool: "new_menu_entry",
        description: "`sofie new menu`: links a parameterless `get` page of the module's area from \
                      the area's menu in `src/areas/<area>/menu.rs`, declaring that file and \
                      turning the `menu` feature on for the module; then `sofie gen` and \
                      `sofie check`.",
        arguments: &[
            MODULE,
            required("area", "AREA", "The area the page is routed in, as `web`."),
            required(
                "handler",
                "handler",
                "The page's handler, as routed in the area.",
            ),
            required("label", "LABEL", "The entry's text, as `Orders`."),
        ],
        build: |values| {
            Ok(NewCode::MenuEntry(NewMenuEntry {
                module: values.text("module"),
                area: values.text("area"),
                handler: values.text("handler"),
                label: values.text("label"),
            }))
        },
    },
];

#[cfg(test)]
mod tests {
    use super::NEW_COMMANDS;
    use crate::new_command::NewCode;

    fn arguments(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    fn command(name: &str) -> &'static super::NewCommand {
        NEW_COMMANDS
            .iter()
            .find(|command| command.name == name)
            .unwrap()
    }

    #[test]
    fn reads_the_arguments_in_order_then_each_option_and_leaves_the_root() {
        let words = arguments(&["billing", "weekly_news", "--list", "news", "shop"]);

        let (code, rest) = command("mail").parse(&words).unwrap();

        let NewCode::Mail { module, name, list } = code else {
            panic!("not a mail");
        };
        assert_eq!(
            (module.as_str(), name.as_str(), list.as_deref()),
            ("billing", "weekly_news", Some("news"))
        );
        assert_eq!(rest, ["shop"]);
        let missing = command("listener").parse(&arguments(&["billing"])).err();
        let unknown = command("account-screen")
            .parse(&arguments(&["customer", "profile"]))
            .err();

        assert!(
            missing
                .unwrap()
                .starts_with("`Event` is missing; usage: sofie new listener")
        );
        assert!(unknown.unwrap().starts_with("`screen` is one of sessions"));
    }

    #[test]
    fn an_area_takes_one_of_the_access_kinds() {
        let (code, _) = command("area")
            .parse(&arguments(&["billing", "console", "permitted"]))
            .unwrap();
        let refused = command("area")
            .parse(&arguments(&["billing", "console", "private"]))
            .err();

        let NewCode::Area { name, access, .. } = code else {
            panic!("not an area");
        };
        assert_eq!(
            (name.as_str(), access),
            ("console", super::AreaAccess::Permitted)
        );
        assert_eq!(
            refused.as_deref(),
            Some("`access` is one of public, owned, permitted")
        );
    }

    #[test]
    fn the_usage_line_shows_each_argument() {
        assert_eq!(
            command("route").usage(),
            "sofie new route MODULE AREA METHOD PATH handler [--screen NAME] [--fields FIELDS] \
             [--read-model Name] [--title TITLE] [--back HANDLER] [ROOT]"
        );
    }

    #[test]
    fn route_fields_are_comma_separated_and_the_read_model_is_a_flag() {
        let words = arguments(&[
            "billing",
            "web",
            "get",
            "/a/{a}",
            "show_a",
            "--fields",
            "title, price",
            "--read-model",
            "ADetail",
        ]);

        let (code, _) = command("route").parse(&words).unwrap();

        let NewCode::Route(route) = code else {
            panic!("not a route");
        };
        assert_eq!(route.fields.unwrap(), ["title", "price"]);
        assert_eq!(route.read_model.as_deref(), Some("ADetail"));
    }
}
