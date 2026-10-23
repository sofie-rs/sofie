import { mount } from "svelte";

import { Runtime } from "../core/index.js";
import App from "./App.svelte";

export function start(screens) {
  const runtime = new Runtime(screens, () => {});
  runtime.start();
  mount(App, { target: document.getElementById("sofie-app"), props: { runtime } });
}
