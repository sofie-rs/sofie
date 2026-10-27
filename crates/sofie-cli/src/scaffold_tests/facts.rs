//! A test is code calling sofie-testing's checks; the checks are the behaviour.

use sofie_check::Screens;
use sofie_model::naming::{snake_case, stored_name};
use sofie_model::{Aggregate, App, Module};

use super::requests::{Making, Requests, writes_factory};
use super::route_facts;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fact {
    /// The aggregate, value, enum or route the fact is about, which its gaps name too.
    pub(crate) subject: String,
    pub(crate) text: String,
    pub(crate) test: String,
    pub(crate) body: String,
    pub(crate) on_app: bool,
}

#[derive(Debug)]
pub(crate) struct Uncovered {
    pub(crate) subject: String,
    pub(crate) reason: String,
}

#[derive(Debug, Default)]
pub(crate) struct ModuleFacts {
    pub(crate) facts: Vec<Fact>,
    pub(crate) uncovered: Vec<Uncovered>,
}

impl ModuleFacts {
    pub(crate) fn uncover(&mut self, subject: &str, reason: String) {
        self.uncovered.push(Uncovered {
            subject: subject.to_owned(),
            reason,
        });
    }
}

const VALUES: [(&str, &str); 19] = [
    ("Coordinate", "sofie::values::Coordinate"),
    ("Currency", "sofie::values::Currency"),
    ("Date", "sofie::values::Date"),
    ("Email", "sofie::values::Email"),
    ("Energy", "sofie::values::Energy"),
    ("Instant", "sofie::core::Instant"),
    ("Length", "sofie::values::Length"),
    ("LocalDateTime", "sofie::values::LocalDateTime"),
    ("Locale", "sofie::values::Locale"),
    ("Money", "sofie::values::Money"),
    ("Percentage", "sofie::values::Percentage"),
    ("PersonName", "sofie::values::PersonName"),
    ("PhoneNumber", "sofie::values::PhoneNumber"),
    ("Slug", "sofie::values::Slug"),
    ("TimeOfDay", "sofie::values::TimeOfDay"),
    ("TimeZone", "sofie::values::TimeZone"),
    ("Url", "sofie::values::Url"),
    ("Weekday", "sofie::values::Weekday"),
    ("Weight", "sofie::values::Weight"),
];

pub(crate) fn module_facts(app: &App, module: &Module, screens: &Screens) -> ModuleFacts {
    let requests = Requests {
        app,
        module,
        screens,
    };
    let mut facts = ModuleFacts::default();
    values(&requests, &mut facts);
    status_enums(&requests, &mut facts);
    for aggregate in &module.code.aggregates {
        aggregate_facts(&requests, aggregate, &mut facts);
    }
    for site in app.route_sites() {
        if std::ptr::eq(site.module, module) {
            route_facts::route(&requests, &site, &mut facts);
        }
    }
    facts
}

fn values(requests: &Requests, facts: &mut ModuleFacts) {
    let code = &requests.module.code;
    let mut seen: Vec<&str> = Vec::new();
    let types = code
        .aggregates
        .iter()
        .flat_map(|aggregate| &aggregate.fields)
        .chain(code.inputs.iter().flat_map(|input| &input.fields))
        .map(
            |field| match (field.ty.name.as_str(), field.ty.arguments.as_slice()) {
                ("Option", [inner]) => inner.name.as_str(),
                (name, _) => name,
            },
        );
    for name in types {
        let Some((_, path)) = VALUES.iter().find(|(value, _)| *value == name) else {
            continue;
        };
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        facts.facts.push(Fact {
            subject: name.to_owned(),
            text: format!("value {name}"),
            test: format!("{}_parses_its_samples_and_round_trips", snake_case(name)),
            body: format!("sofie_testing::assert_value::<{path}>(&app).await;"),
            on_app: true,
        });
    }
}

fn status_enums(requests: &Requests, facts: &mut ModuleFacts) {
    let statuses = requests
        .module
        .code
        .enums
        .iter()
        .filter(|declared| declared.has_transitions());
    for status in statuses {
        let name = status.path.last().map_or("", String::as_str);
        let transitions: Vec<(String, Vec<String>)> = status
            .variants
            .iter()
            .filter(|variant| !variant.to.is_empty())
            .map(|variant| {
                let to = variant
                    .to
                    .iter()
                    .map(|target| stored_name(target))
                    .collect();
                (stored_name(&variant.name), to)
            })
            .collect();
        let moves: Vec<String> = transitions
            .iter()
            .map(|(from, to)| format!("{from}>{}", to.join(",")))
            .collect();
        let listed: Vec<String> = transitions
            .iter()
            .map(|(from, to)| {
                let to: Vec<String> = to.iter().map(|to| format!("{to:?}")).collect();
                format!("({from:?}, &[{}])", to.join(", "))
            })
            .collect();
        facts.facts.push(Fact {
            subject: name.to_owned(),
            text: format!("transitions {name} {}", moves.join(" ")),
            test: format!("{}_moves_only_along_its_transitions", snake_case(name)),
            body: format!(
                "sofie_testing::assert_moves::<{}::{}>(&[{}]);",
                requests.crate_name(),
                status.path.join("::"),
                listed.join(", ")
            ),
            on_app: false,
        });
    }
}

fn aggregate_facts(requests: &Requests, aggregate: &Aggregate, facts: &mut ModuleFacts) {
    let name = &aggregate.name;
    let snake = snake_case(name);
    let path = requests.path(aggregate);
    let derived = !writes_factory(&aggregate.file);
    facts.facts.push(Fact {
        subject: name.clone(),
        text: format!("schema {name}"),
        test: format!("{snake}_table_matches_its_schema"),
        body: format!("sofie_testing::assert_schema::<{path}>(&app).await;"),
        on_app: true,
    });
    let unmade = requests
        .module
        .code
        .values_without_fixture(&aggregate.fields);
    if derived && !unmade.is_empty() {
        let types: Vec<String> = unmade.iter().map(|ty| format!("`{ty}`")).collect();
        facts.uncover(
            name,
            format!(
                "{name}: no fixture, as {} has no `impl sofie::values::Fixture` (`fn \
                 fixture(seed: &mut sofie::values::Seed) -> Self`, drawing a value its `try_new` \
                 takes); write one, then scaffold the tests again",
                types.join(", ")
            ),
        );
        constraints(requests, aggregate, false, &unmade, facts);
        return;
    }
    if derived {
        facts.facts.push(Fact {
            subject: name.clone(),
            text: format!("factory {name} makes a valid record"),
            test: format!("{snake}_is_made_by_its_factory"),
            body: format!("let _: {path} = app.fixture().await;"),
            on_app: true,
        });
    }
    constraints(requests, aggregate, derived, &unmade, facts);
    let made = requests.two_records(aggregate, derived);
    let Some(made) = made else {
        let reason = match requests.creation_permission(name) {
            Some((route, permission)) => format!(
                "{name}: no fixture, as its factory is written by hand and {route}, which creates \
                 it, needs {permission}, a grant made by hand"
            ),
            None => format!(
                "{name}: no fixture, as its factory is written by hand and no route of the module creates it"
            ),
        };
        facts.uncover(name, reason);
        return;
    };
    facts.facts.push(Fact {
        subject: name.clone(),
        text: format!("store {name}"),
        test: format!("{snake}_loads_as_saved_and_a_stale_save_conflicts"),
        body: format!(
            "{}sofie_testing::assert_store::<{path}>(&app, sofie::db::Id::from_uuid(first), sofie::db::Id::from_uuid(second)).await;",
            made.statements
        ),
        on_app: true,
    });
    if !aggregate.exposed.is_empty() {
        lookup(requests, aggregate, &made, facts);
    }
}

/// `fixture` says whether a second record can be made, for `unique`; a boundary needs a
/// `Fixture` of its field's own type, which `unmade` lists as missing.
fn constraints(
    requests: &Requests,
    aggregate: &Aggregate,
    fixture: bool,
    unmade: &[&str],
    facts: &mut ModuleFacts,
) {
    let name = &aggregate.name;
    let snake = snake_case(name);
    let path = requests.path(aggregate);
    for declared in &aggregate.fields {
        let field = &declared.name;
        let types = std::iter::once(&declared.ty).chain(&declared.ty.arguments);
        if types
            .into_iter()
            .any(|ty| unmade.contains(&ty.name.as_str()))
        {
            continue;
        }
        for constraint in &declared.bounds {
            let text = format!("constraint {name}.{field} {constraint}");
            if constraint == "unique" && !fixture {
                facts.uncover(
                    name,
                    format!(
                        "{name}.{field} unique: no fixture to make a second record with, as its \
                         factory is written by hand or a field's type has no `Fixture`"
                    ),
                );
                continue;
            }
            let fact = if constraint == "unique" {
                Fact {
                    subject: name.clone(),
                    text,
                    test: format!("{snake}_{field}_is_unique"),
                    body: format!("sofie_testing::assert_unique::<{path}>(&app, {field:?}).await;"),
                    on_app: true,
                }
            } else {
                Fact {
                    subject: name.clone(),
                    text,
                    test: format!(
                        "{snake}_{field}_keeps_{}",
                        constraint.replace([' ', '-'], "_")
                    ),
                    body: format!(
                        "sofie_testing::assert_boundary({field:?}, {path}::__{}, {constraint:?});",
                        field.to_uppercase()
                    ),
                    on_app: false,
                }
            };
            facts.facts.push(fact);
        }
    }
}

fn lookup(requests: &Requests, aggregate: &Aggregate, made: &Making, facts: &mut ModuleFacts) {
    let Some(contract) = &requests.module.contract else {
        return;
    };
    let name = &aggregate.name;
    let mut exposed = vec!["id".to_owned()];
    exposed.extend(aggregate.exposed.iter().cloned());
    exposed.sort();
    let listed: Vec<String> = exposed.iter().map(|field| format!("{field:?}")).collect();
    facts.facts.push(Fact {
        subject: name.clone(),
        text: format!("lookup {name} {}", exposed.join(",")),
        test: format!("{}_lookup_answers_its_exposed_fields", snake_case(name)),
        body: format!(
            "{}sofie_testing::assert_lookup::<{}, {}::{name}>(&app, &[sofie::db::Id::from_uuid(first), sofie::db::Id::from_uuid(second)], &[{}]).await;",
            made.statements,
            requests.path(aggregate),
            contract.crate_name,
            listed.join(", ")
        ),
        on_app: true,
    });
}
