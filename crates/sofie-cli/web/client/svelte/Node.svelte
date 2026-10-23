<script>
  import { isVoid } from "../core/index.js";
  import Node from "./Node.svelte";

  let { node, slots } = $props();
</script>

{#if typeof node === "string"}{node}{:else if node.slot}{#if slots[node.slot]}{@render slots[node.slot](node.argument)}{/if}{:else if isVoid(node.tag)}<svelte:element this={node.tag} {...node.attributes} />{:else}<svelte:element this={node.tag} {...node.attributes}>{#each node.children as child}<Node node={child} {slots} />{/each}</svelte:element>{/if}
