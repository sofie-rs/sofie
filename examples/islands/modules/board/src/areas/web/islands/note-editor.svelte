<script lang="ts">
  import type { NoteDraft } from "../../../../../../web/gen/types";
  import WordCount from "../../../../web/word-count.svelte";

  let { title, body }: NoteDraft = $props();
  let saved = $state("");

  async function save() {
    const response = await fetch("/notes", {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "application/json" },
      body: JSON.stringify({ title, body }),
    });
    saved = response.ok ? "Saved." : "Not saved.";
  }
</script>

<form class="note-editor" onsubmit={(event) => { event.preventDefault(); save(); }}>
  <input bind:value={title} aria-label="Title" />
  <textarea bind:value={body} aria-label="Note"></textarea>
  <WordCount {body} />
  <button>Save</button>
  <p role="status">{saved}</p>
</form>

<style>
  .note-editor { display: grid; gap: 0.5rem; max-width: 40rem; }
</style>
