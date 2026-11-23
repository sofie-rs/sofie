<script lang="ts">
  import { socket } from "sofie:socket";
  import type { NoteDraft } from "../../../../../../web/gen/types";
  import WordCount from "../../../../web/word-count.svelte";

  let { title, body }: NoteDraft = $props();
  let saved = $state("");
  let latest = $state<NoteDraft>({ title, body });

  // Saved here or in another tab, the latest note arrives as it is written.
  $effect(() =>
    socket.watch<NoteDraft>("/", (answer) => {
      if (answer.status === 200) latest = answer.body;
    }),
  );

  async function save() {
    try {
      const answer = await socket.call("POST", "/notes", { title, body });
      saved = answer.status === 200 ? "Saved." : "Not saved.";
    } catch {
      saved = "Not saved.";
    }
  }
</script>

<form class="note-editor" onsubmit={(event) => { event.preventDefault(); save(); }}>
  <input bind:value={title} aria-label="Title" />
  <textarea bind:value={body} aria-label="Note"></textarea>
  <WordCount {body} />
  <button>Save</button>
  <p role="status">{saved}</p>
  <p class="latest">Latest: {latest.title}</p>
</form>

<style>
  .note-editor { display: grid; gap: 0.5rem; max-width: 40rem; }
</style>
