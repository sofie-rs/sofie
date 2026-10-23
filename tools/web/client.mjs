// The client build, run by `sofie` (an app's build script and `sofie dev`):
// `node client.mjs <plan.json>`. The plan names the output directory, the mode,
// the area, its client (`svelte` or `react`) and each screen's key and compiled
// component; the area gets `<area>.js` and `<area>.css` there.
import { fileURLToPath } from "node:url";

import { bundle, currencies, entry, readPlan } from "./shared.mjs";

const { plan, development } = await readPlan();
const adapter = `client/${plan.client}`;

await bundle({
  out: plan.out,
  area: plan.area,
  contents: entry(plan.screens.map((screen) => [screen.key, screen.file]), `${adapter}/start.js`, "start"),
  sourcefile: `${plan.area}-client.js`,
  development,
  options: {
    alias: { [`sofie:${plan.client}`]: fileURLToPath(new URL(`${adapter}/index.js`, import.meta.url)) },
    define: { "process.env.NODE_ENV": JSON.stringify(development ? "development" : "production") },
    jsx: "automatic",
    plugins: [currencies(plan.currencies)],
  },
});
