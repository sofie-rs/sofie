//! Screens compiled into a client framework's components when the app is built.

mod emit;
mod script;
mod tree;

use std::fmt;

use crate::{Components, Template, TemplateError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    Svelte,
    React,
}

impl Client {
    /// As the web toolchain names the client's adapter.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Svelte => "svelte",
            Self::React => "react",
        }
    }

    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Svelte => "svelte",
            Self::React => "jsx",
        }
    }
}

impl fmt::Display for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Svelte => "Svelte",
            Self::React => "React",
        })
    }
}

/// One screen as the source of a component taking `data` and `view` props.
pub fn compile_client(
    name: &str,
    source: &str,
    components: &Components,
    client: Client,
) -> Result<String, TemplateError> {
    let template = Template::parse_with(name, source, components)?;
    tree::tree(&template.ops)
        .and_then(|nodes| emit::Emitter::new(client).screen(&nodes))
        .map_err(|message| {
            TemplateError::unlocated(name, format!("compiling for {client}: {message}"))
        })
}

#[cfg(test)]
mod tests {
    use super::{Client, compile_client};
    use crate::Components;

    const SOURCE: &str = "<Section title=\"Tom &amp; Jerry\">\
        <ul class=\"list {data.kind}\">{#each data.items as item, index}\
        <li><a href={item.url}>{item.name}</a></li>{/each}</ul>\
        {#if data.empty}<p>None {\"{x}\"}</p>{:else}<input required>{/if}\
        <Table id=\"t\" of={data.rows} columns=\"name\" as=\"row\">\
        <Action post=\"/r/{row.id}\" label=\"Go\" /></Table></Section>";

    fn compiled(client: Client) -> String {
        compile_client("screen", SOURCE, &Components::default(), client).unwrap()
    }

    #[test]
    fn svelte_keeps_markup_and_writes_blocks_and_snippets() {
        let svelte = compiled(Client::Svelte);

        for expected in [
            "props={{\"title\": S.text(\"Tom & Jerry\")}}",
            "<ul class=\"list {S.text(data?.kind)}\">",
            "{#each S.items(data?.items) as s0, s1}<li><a href=\"{S.url(S.text(s0?.url))}\">",
            "{S.text(s0?.name)}</a></li>{/each}",
            "{#if S.truthy(data?.empty)}<p>None {S.text(\"{x}\")}</p>{:else}<input required=\"\">{/if}",
            "{#snippet row(s0)}<Catalog {view} name=\"Action\" props={{\"post\": \
             S.join([\"/r/\", S.urlPart(s0?.id)]), \"label\": S.text(\"Go\")}} />{/snippet}",
        ] {
            assert!(svelte.contains(expected), "{expected}\n{svelte}");
        }
    }

    #[test]
    fn react_writes_jsx_with_keys_properties_and_exact_text() {
        let react = compiled(Client::React);

        for expected in [
            "<ul className={\"list \" + S.text(data?.kind)}>",
            "{S.items(data?.items).map((s0, s1) => (<Fragment key={s1}><li>",
            "<a href={S.url(S.text(s0?.url))}>{S.text(s0?.name)}</a>",
            "{S.truthy(data?.empty) ? (<><p>{\"None \"}{S.text(\"{x}\")}</p></>) : \
             (<><input required={true} /></>)}",
            "row={(s0) => (<><Catalog view={view} name=\"Action\"",
        ] {
            assert!(react.contains(expected), "{expected}\n{react}");
        }
    }

    #[test]
    fn an_action_passes_its_posted_fields_as_values() {
        let source = "{#each data.slots as slot}<Action post=\"/slots/book\" label=\"Book\" \
            with={{slot: slot.id, seats: 2}} />{/each}";

        let svelte =
            compile_client("slots", source, &Components::default(), Client::Svelte).unwrap();

        assert!(
            svelte.contains("\"with\": {\"slot\": s0?.id, \"seats\": 2}"),
            "{svelte}"
        );
    }

    #[test]
    fn a_svelte_screen_s_title_names_the_page_through_its_head() {
        let svelte = compile_client(
            "order",
            "<title>Order {data.id}</title><p>x</p>",
            &Components::default(),
            Client::Svelte,
        )
        .unwrap();

        assert!(
            svelte.contains(
                "<svelte:head><title>Order {S.text(data?.id)}</title></svelte:head><p>x</p>"
            ),
            "{svelte}"
        );
    }

    #[test]
    fn written_pages_stay_in_html_areas() {
        let error = compile_client(
            "page",
            "<Markdown text={data.body} />",
            &Components::default(),
            Client::Svelte,
        )
        .unwrap_err();

        assert!(error.message.contains("only in HTML areas"), "{error}");
    }

    #[test]
    fn page_script_attributes_are_kept_for_the_client_runtime() {
        let svelte = compile_client(
            "passkeys",
            "<button type=\"button\" data-sofie-passkey=\"create\" data-options={data.options} \
             data-post=\"passkeys\">Add</button><form method=\"post\" action={data.action} \
             data-sofie-autosubmit=\"true\"></form>",
            &Components::default(),
            Client::Svelte,
        )
        .unwrap();

        for expected in [
            "<button type=\"button\" data-sofie-passkey=\"create\" \
             data-options=\"{S.text(data?.options)}\" data-post=\"passkeys\">Add</button>",
            "data-sofie-autosubmit=\"true\"",
        ] {
            assert!(svelte.contains(expected), "{expected}\n{svelte}");
        }
    }
}
