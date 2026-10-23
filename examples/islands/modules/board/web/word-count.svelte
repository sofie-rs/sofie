<script lang="ts">
  let { body = "" }: { body?: string } = $props();
  let words = $derived(body.split(/\s+/).filter(Boolean).length);
</script>

<span class="word-count">{words} words</span>
