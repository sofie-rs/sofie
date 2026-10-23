<script>
  import { component } from "../core/index.js";
  import Node from "./Node.svelte";

  let { view, name, props, children, row } = $props();

  const tree = $derived(
    component(name, props, view, { children: children !== undefined, row: row !== undefined }),
  );
</script>

{#if tree}<Node node={tree} slots={{ children, row }} />{/if}
