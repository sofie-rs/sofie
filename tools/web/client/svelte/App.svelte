<script>
  import { untrack } from "svelte";

  import { S } from "../core/index.js";

  let { runtime } = $props();

  // The runtime is made once per page; only what it shows changes.
  const shown = untrack(() => runtime);
  let state = $state.raw(shown.state);
  shown.onChange = (next) => {
    state = next;
  };

  const Screen = $derived(shown.screens[state.envelope.screen]);
  const view = $derived({
    ...state.envelope.view,
    instants: S.instantFields(state.envelope.data, state.envelope.instants),
    language: state.envelope.language,
    submitted: state.submitted,
    runtime: shown,
  });
  // Effects run once the screen is in the document.
  $effect(() => {
    shown.rendered(state.envelope);
  });
</script>

<div id="sofie-notice" role="status" aria-live="polite">{#each state.notice as message}<p>{message}</p>{/each}</div>
{#if state.envelope.navigation.length > 0}<nav class="sofie-nav"><ul>{#each state.envelope.navigation as entry}<li><a href={entry.path} aria-current={entry.current ? "page" : undefined}>{entry.label}</a></li>{/each}</ul></nav>{/if}
<Screen data={state.envelope.data} {view} />
