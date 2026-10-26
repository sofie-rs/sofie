use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use sofie_check::Screens;
use sofie_engine::{Client, Components, compile_client};
use sofie_model::{App, AreaClient, Identity, KnownArea, ShippedScreen};

use crate::cargo::Diagnostics;
use crate::child_process;
use crate::gen_command::{GenError, io_error, readable_screen};
use crate::islands::{IslandMode, TOOLCHAIN};
use crate::wiring::embedded_screens;

/// An area whose screens are compiled for a client framework, with the screens it has.
pub(crate) struct ClientArea<'app> {
    pub(crate) area: String,
    pub(crate) client: Client,
    pub(crate) screens: Vec<(String, &'app Path)>,
    /// The framework screens mounted in the area that compile for its client, each key with
    /// its component; any other stays an HTML page.
    pub(crate) mounted: Vec<(String, String)>,
}

impl ClientArea<'_> {
    pub(crate) fn keys(&self) -> impl Iterator<Item = &str> {
        let own = self.screens.iter().map(|(key, _)| key.as_str());
        own.chain(self.mounted.iter().map(|(key, _)| key.as_str()))
    }
}

pub(crate) fn client_areas(app: &App) -> Vec<ClientArea<'_>> {
    app.areas()
        .into_iter()
        .filter_map(|known| {
            let client = match known.definition?.client {
                AreaClient::Html | AreaClient::React => return None,
                AreaClient::Svelte => Client::Svelte,
            };
            let screens: Vec<(String, &Path)> = app
                .modules
                .iter()
                .flat_map(embedded_screens)
                .filter(|(key, _)| key.split('/').nth(1) == Some(known.name))
                .collect();
            let mounted = mounted_components(app, &known, client, &screens);
            Some(ClientArea {
                area: known.name.to_owned(),
                client,
                screens,
                mounted,
            })
        })
        .collect()
}

/// A framework screen is not the app's to fix, so one a client cannot render (a written page)
/// stays an HTML page instead of failing the build. An app's own template under the same key
/// replaces it.
fn mounted_components(
    app: &App,
    area: &KnownArea<'_>,
    client: Client,
    screens: &[(String, &Path)],
) -> Vec<(String, String)> {
    let Some(identity) = &app.identity else {
        return Vec::new();
    };
    let with_roles = Identity {
        shipped: [identity.shipped.clone(), role_screens(identity, area)].concat(),
        ..identity.clone()
    };
    with_roles
        .mounted_screens(area)
        .into_iter()
        .filter(|mounted| !screens.iter().any(|(key, _)| *key == mounted.key))
        .filter_map(|mounted| {
            let component = compile_client(
                &mounted.key,
                &mounted.screen.source,
                &Components::default(),
                client,
            )
            .ok()?;
            Some((mounted.key, component))
        })
        .collect()
}

/// sofie-identity renders its role screens from `Role`'s and `RoleGrant`'s descriptions, and
/// keeps the same source as template files, each handler's under its screen's name.
const ROLE_SCREENS: [(&str, &str); 6] = [
    ("list_roles", "role_index"),
    ("show_role", "role_show"),
    ("new_role", "role_create"),
    ("edit_role", "role_edit"),
    ("list_role_grants", "role_grant_index"),
    ("new_role_grant", "role_grant_create"),
];
const ROLE_TEMPLATES: &str = "src/permitted/screens";
/// Where a role template's links name the area's prefix, as the rendered source does.
const PREFIX_PLACEHOLDER: &str = "{prefix}";

/// An area whose prefix the model cannot read, or a sofie-identity older than the files, keeps
/// its role screens as HTML pages, as a screen a client cannot compile does.
fn role_screens(identity: &Identity, area: &KnownArea<'_>) -> Vec<ShippedScreen> {
    let Some(prefix) = area.prefix() else {
        return Vec::new();
    };
    ROLE_SCREENS
        .iter()
        .filter_map(|(handler, name)| {
            let file = identity
                .directory
                .join(ROLE_TEMPLATES)
                .join(format!("{name}.html"));
            let template = fs::read_to_string(&file).ok()?;
            Some(ShippedScreen {
                handler: (*handler).to_owned(),
                name: (*name).to_owned(),
                file,
                source: template.replace(PREFIX_PLACEHOLDER, prefix),
            })
        })
        .collect()
}

#[must_use]
pub(crate) fn has_clients(app: &App) -> bool {
    !client_areas(app).is_empty()
}

/// Each client area's bundle, `<area>.js` and `<area>.css` in `out`.
pub(crate) fn build_clients(
    app: &App,
    screens: &Screens,
    out: &Path,
    mode: IslandMode,
    diagnostics: Diagnostics,
) -> Result<Vec<String>, GenError> {
    let areas = client_areas(app);
    if areas.is_empty() {
        return Ok(Vec::new());
    }
    check_toolchain()?;
    for area in &areas {
        let plan = write_area(area, screens, out, mode)?;
        run_node("client.mjs", &plan, "the client build", diagnostics)?;
    }
    Ok(areas.into_iter().map(|area| area.area).collect())
}

fn write_area(
    area: &ClientArea<'_>,
    screens: &Screens,
    out: &Path,
    mode: IslandMode,
) -> Result<PathBuf, GenError> {
    let directory = out.join(&area.area);
    fs::create_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
    let mut components = Vec::new();
    for (key, file) in &area.screens {
        let source = &readable_screen(screens, file)?.source;
        let component =
            compile_client(key, source, screens.components(), area.client).map_err(|error| {
                GenError::Screen {
                    path: file.to_path_buf(),
                    message: error.to_string(),
                }
            })?;
        components.push((key, component));
    }
    let mounted = area
        .mounted
        .iter()
        .map(|(key, component)| (key, component.clone()));
    let mut planned = Vec::new();
    for (index, (key, component)) in components.into_iter().chain(mounted).enumerate() {
        let path = directory.join(format!("screen{index}.{}", area.client.extension()));
        fs::write(&path, component).map_err(|source| io_error(&path, source))?;
        planned.push(json!({ "key": key, "file": absolute(&path) }));
    }
    let plan = directory.join("plan.json");
    let text = json!({
        "mode": mode.name(),
        "out": absolute(out),
        "area": area.area,
        "client": area.client.name(),
        "screens": planned,
        "currencies": currencies(),
    })
    .to_string();
    fs::write(&plan, text).map_err(|source| io_error(&plan, source))?;
    Ok(plan)
}

/// The toolchain files a client bundle is built from; the screens are watched with `modules/`.
pub(crate) fn client_sources() -> Vec<PathBuf> {
    ["client.mjs", "shared.mjs", "package-lock.json", "client"]
        .map(|file| Path::new(TOOLCHAIN).join(file))
        .to_vec()
}

/// Each currency's minor digits and the symbol the languages write it with, from the same
/// tables the server formats with.
fn currencies() -> serde_json::Value {
    sofie_values::Currency::all()
        .map(|currency| {
            let code = currency.as_str().to_owned();
            let symbol = sofie_engine::CURRENCY_SYMBOLS
                .iter()
                .find(|(known, _)| *known == code)
                .map(|(_, symbol)| *symbol);
            (code, json!([currency.minor_digits(), symbol]))
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

pub(crate) fn check_toolchain() -> Result<(), GenError> {
    let toolchain = Path::new(TOOLCHAIN);
    if toolchain.join("node_modules").is_dir() {
        return Ok(());
    }
    Err(GenError::Usage(format!(
        "the app builds islands or client areas, which need the framework's web toolchain: run \
         `npm ci` in {}",
        toolchain.display()
    )))
}

pub(crate) fn run_node(
    script: &str,
    plan: &Path,
    what: &str,
    diagnostics: Diagnostics,
) -> Result<(), GenError> {
    let mut node = Command::new("node");
    node.arg(Path::new(TOOLCHAIN).join(script)).arg(plan);
    let output =
        child_process::output(diagnostics.process_group(), &mut node).map_err(|source| {
            GenError::Usage(format!("running Node for {what}: {source}; install Node"))
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(GenError::App(format!(
        "{what} failed with {}:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// Node resolves the plan's files from wherever it runs.
pub(crate) fn absolute(file: &Path) -> String {
    std::path::absolute(file)
        .unwrap_or_else(|_| file.to_path_buf())
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use sofie_model::testing::{app, define_area, module};
    use sofie_model::{AreaClient, ListedRoute, RouteMount};

    use super::*;

    #[test]
    fn a_mounted_framework_screen_joins_the_bundle_unless_a_client_cannot_render_it() {
        let mut demo = board_app();
        demo.identity = Some(Identity {
            routes: vec![
                identity_route("sessions::routes", "list_sessions"),
                identity_route("content::routes", "show_page"),
            ],
            mounts: vec![
                identity_mount("sessions::routes"),
                identity_mount("content::routes"),
            ],
            shipped: vec![
                shipped(
                    "list_sessions",
                    "sessions",
                    "<Section title=\"Sessions\"></Section>",
                ),
                shipped("show_page", "page", "<Markdown text={data.body} />"),
            ],
            ..Identity::default()
        });

        let areas = client_areas(&demo);

        let keys: Vec<&str> = areas[0].keys().collect();
        assert_eq!(keys, ["sofie_identity/board/sessions"]);
        assert!(areas[0].mounted[0].1.contains("name=\"Section\""));
    }

    #[test]
    fn mounted_role_screens_join_the_bundle_with_the_area_s_prefix() {
        let mut demo = board_app();
        demo.identity = Some(Identity {
            directory: identity_directory(),
            routes: vec![
                identity_route("permitted_routes", "list_roles"),
                identity_route("permitted_routes", "list_role_grants"),
            ],
            mounts: vec![identity_mount("permitted_routes")],
            ..Identity::default()
        });

        let areas = client_areas(&demo);

        let keys: Vec<&str> = areas[0].keys().collect();
        assert_eq!(
            keys,
            [
                "sofie_identity/board/role_index",
                "sofie_identity/board/role_grant_index"
            ]
        );
        assert!(
            areas[0].mounted[0].1.contains("/account/roles/new"),
            "{}",
            areas[0].mounted[0].1
        );
    }

    #[test]
    fn sofie_identity_s_account_and_role_screens_compile_for_svelte() {
        let account_screens = [
            "src/account_data/screen.html",
            "src/impersonation/screen.html",
            "src/oauth/connections.html",
            "src/passkeys/screen.html",
            "src/permitted/screens/flags.html",
            "src/permitted/screens/settings.html",
            "src/permitted/screens/shield.html",
            "src/sessions/screen.html",
            "src/tokens/screen.html",
            "src/two_factor/screen.html",
        ];
        let role_screens = ROLE_SCREENS
            .iter()
            .map(|(_, name)| format!("{ROLE_TEMPLATES}/{name}.html"));
        let screens = account_screens
            .into_iter()
            .map(str::to_owned)
            .chain(role_screens);

        let refused: Vec<String> = screens
            .filter(|screen| {
                let source = fs::read_to_string(identity_directory().join(screen))
                    .unwrap()
                    .replace(PREFIX_PLACEHOLDER, "/console");
                compile_client(screen, &source, &Components::default(), Client::Svelte).is_err()
            })
            .collect();

        assert_eq!(refused, Vec::<String>::new());
    }

    fn board_app() -> App {
        let mut notes = module("notes", "/app/modules/notes");
        define_area(&mut notes, "board");
        notes.defined_areas[0].client = AreaClient::Svelte;
        app("/app", vec![notes])
    }

    fn identity_directory() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../sofie-identity")
    }

    fn identity_mount(mount: &str) -> RouteMount {
        RouteMount {
            mount: mount.to_owned(),
            area_type: "Member".to_owned(),
        }
    }

    fn identity_route(mount: &str, handler: &str) -> ListedRoute {
        ListedRoute {
            mount: mount.to_owned(),
            method: "GET".to_owned(),
            path: format!("/{handler}"),
            handler: handler.to_owned(),
            permission: None,
        }
    }

    fn shipped(handler: &str, name: &str, source: &str) -> ShippedScreen {
        ShippedScreen {
            handler: handler.to_owned(),
            name: name.to_owned(),
            file: format!("{name}.html").into(),
            source: source.to_owned(),
        }
    }
}

/// Each screen compiled for Svelte and rendered by Svelte on the server is held to the HTML the
/// engine renders from the same template, data and descriptions; it needs Node and `npm ci` in
/// the web toolchain.
#[cfg(all(test, feature = "slow-tests"))]
mod conformance {
    use serde_json::{Value, json};
    use sofie_core::listing::{CONTROLS, ERRORS, FILTER, HAS_NEXT, ITEMS, PAGE, SIZE, SORT};
    use sofie_core::show::Fields;
    use sofie_core::{Constraint, DisplayZone, Instant, Show, Shown};
    use sofie_engine::{
        Client, Components, Template, compile_client, instant_fields, plain_client_view,
    };
    use sofie_model::testing::TemporaryDirectory;
    use sofie_values::{Control, FormField, Input, Part};

    use super::{TOOLCHAIN, currencies};

    const FIELDS: &[FormField] = &[
        FormField {
            name: "title",
            input: Input::typed(&[("type", "text")]),
            required: true,
            constraints: &[Constraint::MinLength(2), Constraint::MaxLength(80)],
        },
        FormField {
            name: "price",
            input: Input::new(Control::Money),
            required: true,
            constraints: &[],
        },
        FormField {
            name: "pinned",
            input: Input::new(Control::Checkbox),
            required: false,
            constraints: &[],
        },
        FormField {
            name: "kind",
            input: Input::select(&["NOTE", "TASK"]),
            required: false,
            constraints: &[],
        },
        FormField {
            name: "body",
            input: Input::new(Control::Textarea),
            required: false,
            constraints: &[],
        },
        FormField {
            name: "share",
            input: Input::new(Control::Percentage),
            required: false,
            constraints: &[],
        },
        FormField {
            name: "secret",
            input: Input::new(Control::Password),
            required: false,
            constraints: &[],
        },
        FormField {
            name: "at",
            input: Input::typed(&[("type", "datetime-local")]),
            required: false,
            constraints: &[],
        },
        FormField {
            name: "address",
            input: Input::new(Control::Parts(&[
                Part {
                    name: "street",
                    attributes: &[("type", "text")],
                    options: &[],
                },
                Part {
                    name: "country",
                    attributes: &[],
                    options: &["DE", "TR"],
                },
            ])),
            required: false,
            constraints: &[],
        },
    ];

    fn fixtures() -> Vec<Fixture> {
        vec![
            plain(
                "table",
                "<Section title=\"Notes &amp; tasks\"><Table id=\"notes\" of={data.rows} \
                 columns=\"title price count at\" as=\"row\" link=\"/notes/{row.id}\">{#if row.done}<strong>Done</strong>\
                 {:else}<Action post=\"/notes/{row.id}/done\" label=\"Done\" target=\"notes\" \
                 when={row.open} />{/if}</Table><Table id=\"none\" of={data.none} \
                 columns=\"title\" /></Section>",
                json!({
                    "rows": [
                        {"at": "2026-10-11T22:30:00Z", "count": 3, "done": false, "id": "n1", "open": true,
                         "price": {"currency": "EUR", "minor": 1250}, "title": "<b>First</b>"},
                        {"at": "2026-01-02T03:04:05Z", "count": 2.5, "done": true, "id": "n2", "open": false,
                         "price": {"currency": "JPY", "minor": -1500}, "title": "Second"},
                        {"at": null, "count": null, "done": false, "id": "n3", "open": false,
                         "price": null, "title": "Third"},
                    ],
                    "none": [],
                }),
            ),
            plain(
                "markup",
                "<div class=\"box {data.kind}\" data-id=\"x-{data.id}\"><h1>{data.title}</h1>\
                 <ul>{#each data.items as item, index}<li>{index}: <a href={item.url}>{item.name}\
                 </a></li>{/each}</ul>{#if data.empty}<p>Nothing</p>{:else}<p>Some</p>{/if}\
                 <Money value={data.total} /><img src={data.image} alt=\"Picture\">\
                 <a href=\"/notes/{data.slug}?n={data.id}\">Slug</a></div>",
                json!({
                    "empty": [],
                    "id": 7,
                    "image": "javascript:alert(1)",
                    "items": [
                        {"name": "Safe", "url": "https://example.com/a?b=1&c=2"},
                        {"name": "Unsafe", "url": "data:text/html,x"},
                    ],
                    "kind": "wide",
                    "slug": "a b/c&d",
                    "title": "Hello",
                    "total": {"currency": "KWD", "minor": 1005},
                }),
            ),
            plain(
                "form",
                "<Form id=\"write\" post=\"/notes\" target=\"notes\"><Field name=\"title\" \
                 value={data.title} /><Field name=\"price\" value={data.price} /><Field \
                 name=\"pinned\" value={data.pinned} /><Field name=\"kind\" value={data.kind} />\
                 <Field name=\"body\" value={data.body} /><Field name=\"share\" \
                 value={data.share} /><Field name=\"address\" value={data.address} />\
                 <Submit label=\"Save\" /></Form><Form id=\"bare\" post=\"/bare\"><Field \
                 name=\"price\" currency=\"EUR\" /><Submit label=\"Go\" /></Form><Form \
                 id=\"choose\" post=\"/choose\"><Field name=\"kind\" options={data.none} />\
                 <Submit label=\"Pick\" /></Form>",
                json!({
                    "address": {"country": "TR", "street": "Main 1"},
                    "body": "Line one\nLine two",
                    "kind": "TASK",
                    "none": null,
                    "pinned": true,
                    "price": {"currency": "EUR", "minor": 990},
                    "share": -1250,
                    "title": "Draft",
                }),
            ),
            listing(),
            plain(
                "picking",
                "<ul>{#each data.slots as slot}<li><Action id=\"book-{slot.id}\" \
                 post=\"/slots/book\" label=\"Book\" with={{slot: slot.id, note: \"a&b\"}} />\
                 </li>{/each}</ul><Table id=\"slots\" of={data.slots} columns=\"time\" \
                 as=\"row\"><Action post=\"/slots/{row.id}/hold\" label=\"Hold\" \
                 target=\"slots\" with={{slot: row.id, seats: 2, gone: row.gone}} /></Table>",
                json!({
                    "slots": [
                        {"id": 7, "time": "09:00"},
                        {"id": "a\"<b>", "time": "10:00"},
                    ],
                }),
            ),
            plain(
                "layout",
                "<Grid><Card href=\"/a\"><p>Linked</p></Card><Card id=\"plain\"><p>Plain</p>\
                 </Card></Grid><Island name=\"note-chart\" props={data.chart} />",
                json!({"chart": {"points": [1, 2], "title": "Chart"}}),
            ),
            plain(
                "page_script",
                "<button type=\"button\" data-sofie-passkey=\"create\" data-options={data.options} \
                 data-post=\"passkeys\">Add a passkey</button><form method=\"post\" \
                 action={data.action} data-sofie-autosubmit=\"true\"><button \
                 type=\"submit\">Continue</button></form>",
                json!({
                    "action": "/sign-in/oauth/callback",
                    "options": "{\"challenge\":\"AAEC\",\"rp\":{\"name\":\"Shop & Co\"}}",
                }),
            ),
            moments(),
        ]
    }

    /// Keyed as the server answers a listing, so the client reading any other key renders apart
    /// from the engine. A refused filter is plain text or, as the server sends it, a record.
    fn listing() -> Fixture {
        plain(
            "listing",
            "<Table id=\"orders\" of={data} columns=\"status total\" sort=\"total\" \
             filter=\"status placed_at\" as=\"order\" link={order.url} />",
            json!({
                CONTROLS: {"placed_at": {"type": "datetime-local"}},
                ERRORS: {
                    "placed_at": "is repeated: +02:00 or +01:00",
                    "status": {"arguments": {"max": "<5>"}, "key": "validation.max",
                               "message": "must be at most <5>"},
                    "total.gt": "<x>",
                    "total.lt": {"arguments": {"max": "9"}, "key": "validation.max",
                                 "message": "must be at most 9"},
                },
                FILTER: {"placed_at": "2026-10-25T02:30", "status": "open now"},
                HAS_NEXT: true,
                ITEMS: [
                    {"status": "open", "total": {"currency": "USD", "minor": 100},
                     "url": "/orders/1"},
                    {"status": "held", "total": null, "url": "javascript:alert(1)"},
                ],
                PAGE: 2,
                SIZE: 50,
                SORT: "-total",
            }),
        )
    }

    type Fixture = (&'static str, &'static str, Box<dyn Show>, Value);

    fn plain(name: &'static str, source: &'static str, data: Value) -> Fixture {
        (name, source, Box::new(data.clone()), data)
    }

    /// Typed, as a read model is: the engine tells its instants from text written like one, and
    /// the client is told where they are. Two rows fall in Berlin's repeated hour, one each side of
    /// its shift; one has a fraction.
    fn moments() -> Fixture {
        let instant = |text| Instant::parse(text).unwrap();
        let moments = Moments {
            rows: vec![
                Moment {
                    at: Some(instant("2026-10-25T00:30:00Z")),
                    note: "2026-10-11T22:30:00Z",
                },
                Moment {
                    at: Some(instant("2026-10-25T01:30:00Z")),
                    note: "",
                },
                Moment { at: None, note: "" },
            ],
            fold: instant("2026-10-25T01:30:00Z"),
            exact: instant("2026-10-11T22:30:15.5Z"),
        };
        let rows: Vec<Value> = moments
            .rows
            .iter()
            .map(|row| json!({"at": row.at, "note": row.note}))
            .collect();
        let data = json!({"rows": rows, "fold": moments.fold, "exact": moments.exact});
        (
            "instants",
            "<Table id=\"moments\" of={data.rows} columns=\"at note\" /><Form id=\"when\" \
             post=\"/when\"><Field name=\"at\" value={data.fold} /><Submit label=\"Save\" /></Form>\
             <Form id=\"exact\" post=\"/exact\"><Field name=\"at\" value={data.exact} /><Submit \
             label=\"Save\" /></Form>",
            Box::new(moments),
            data,
        )
    }

    struct Moment {
        at: Option<Instant>,
        note: &'static str,
    }

    impl Fields for Moment {
        fn name(&self, index: usize) -> Option<&str> {
            ["at", "note"].get(index).copied()
        }

        fn get(&self, name: &str) -> Option<Shown<'_>> {
            match name {
                "at" => Some(self.at.show()),
                "note" => Some(Shown::Text(self.note)),
                _ => None,
            }
        }
    }

    impl Show for Moment {
        fn show(&self) -> Shown<'_> {
            Shown::Record(self)
        }
    }

    struct Moments {
        rows: Vec<Moment>,
        fold: Instant,
        exact: Instant,
    }

    impl Fields for Moments {
        fn name(&self, index: usize) -> Option<&str> {
            ["rows", "fold", "exact"].get(index).copied()
        }

        fn get(&self, name: &str) -> Option<Shown<'_>> {
            match name {
                "rows" => Some(self.rows.show()),
                "fold" => Some(self.fold.show()),
                "exact" => Some(self.exact.show()),
                _ => None,
            }
        }
    }

    impl Show for Moments {
        fn show(&self) -> Shown<'_> {
            Shown::Record(self)
        }
    }

    fn engine_html(name: &str, source: &str, data: &dyn Show) -> (String, Value) {
        let mut template = Template::parse(name, source).unwrap();
        let forms: &[&str] = match name {
            "form" => &["write", "choose"],
            "instants" => &["when", "exact"],
            _ => &[],
        };
        for form in forms {
            template.describe_form(form, FIELDS).unwrap();
        }
        (template.render(data).unwrap(), plain_client_view(&template))
    }

    /// What the two renderers may write differently and a browser reads alike.
    fn normalized(html: &str) -> String {
        let mut text = String::with_capacity(html.len());
        let mut rest = html;
        while let Some(start) = rest.find("<!--") {
            text.push_str(&rest[..start]);
            rest = rest[start..]
                .find("-->")
                .map_or("", |end| &rest[start + end + 3..]);
        }
        text.push_str(rest);
        let mut words = text
            .replace("&gt;", ">")
            .replace("&#39;", "'")
            .replace("=\"\"", "");
        while let Some(start) = words.find(" hx-") {
            let quoted = words[start..].find("=\"").unwrap() + start + 2;
            let end = words[quoted..].find('"').unwrap() + quoted + 1;
            words.replace_range(start..end, "");
        }
        words
            .replace("/>", ">")
            .replace(">\n", ">")
            .replace("> <", "><")
    }

    #[test]
    fn the_browser_runtimes_pass_their_node_checks() {
        for check in [
            "telemetry-check.mjs",
            "engine-check.mjs",
            "format-check.mjs",
            "navigation-check.mjs",
        ] {
            let output = std::process::Command::new("node")
                .arg(std::path::Path::new(TOOLCHAIN).join("client").join(check))
                .output()
                .unwrap();

            assert!(
                output.status.success(),
                "{check}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn svelte_screens_render_what_the_engine_renders() {
        // A zone with a backward shift, installed before any instant is read, so an instant in its
        // repeated hour is held to the engine too.
        DisplayZone::named("Europe/Berlin")
            .unwrap()
            .install()
            .unwrap();
        let directory = TemporaryDirectory::new("sofie-conformance");
        let fixtures = fixtures();
        let mut expected = Vec::new();
        for (name, source, typed, data) in &fixtures {
            let (html, view) = engine_html(name, source, typed.as_ref());
            expected.push((*name, html));
            let component =
                compile_client(name, source, &Components::default(), Client::Svelte).unwrap();
            std::fs::write(directory.path(&format!("{name}.svelte")), component).unwrap();
            let instants = instant_fields(typed.show(), data);
            let props = json!({ "data": data, "view": view, "instants": instants });
            std::fs::write(directory.path(&format!("{name}.json")), props.to_string()).unwrap();
        }
        std::fs::write(directory.path("currencies.json"), currencies().to_string()).unwrap();

        let output = std::process::Command::new("node")
            .arg(std::path::Path::new(TOOLCHAIN).join("client/conformance.mjs"))
            .arg(directory.root())
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rendered: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        for (name, html) in expected {
            let svelte = rendered
                .iter()
                .find(|line| line["name"] == name)
                .and_then(|line| line["html"].as_str())
                .unwrap_or_default();
            assert_eq!(normalized(svelte), normalized(&html), "screen `{name}`");
        }
    }
}
