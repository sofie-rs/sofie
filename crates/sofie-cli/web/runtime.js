// The element is ours rather than Svelte's `customElement`, so props arrive as data, never through
// attributes, and hot reload can remount a component without redefining its tag.
import { mount, unmount } from "svelte";

const registry = (globalThis.sofieIslands ??= { components: new Map(), elements: new Set() });

export function define(components) {
  const replaced = new Set();
  for (const [tag, component] of Object.entries(components)) {
    if (registry.components.has(tag)) {
      replaced.add(tag);
    }
    registry.components.set(tag, { component, mount, unmount });
    if (!customElements.get(tag)) {
      customElements.define(tag, class extends Island {});
    }
  }
  for (const element of registry.elements) {
    if (replaced.has(element.localName)) {
      element.unmountIsland();
      element.mountIsland();
    }
  }
}

class Island extends HTMLElement {
  connectedCallback() {
    registry.elements.add(this);
    this.mountIsland();
  }

  disconnectedCallback() {
    registry.elements.delete(this);
    this.unmountIsland();
  }

  // A client area's framework may connect the element before it appends the props.
  mountIsland(retried = false) {
    const { component, mount, unmount } = registry.components.get(this.localName);
    const data = this.querySelector(':scope > script[type="application/json"]');
    if (!data && !retried) {
      queueMicrotask(() => this.isConnected && !this.mounted && this.mountIsland(true));
      return;
    }
    const props = data ? JSON.parse(data.textContent) : {};
    this.mounted = { instance: mount(component, { target: this, props }), unmount };
  }

  unmountIsland() {
    if (this.mounted) {
      this.mounted.unmount(this.mounted.instance);
      this.mounted = undefined;
    }
  }
}
