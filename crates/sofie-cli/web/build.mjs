// The island build, run by `sofie` (an app's build script and `sofie dev`):
// `node build.mjs <plan.json>`. The plan names the output directory, the
// mode and, per area, each island's tag and Svelte file; every area gets
// `<area>.js` and `<area>.css` there.
import { bundle, entry, readPlan } from "./shared.mjs";

const { plan, development } = await readPlan();

for (const area of plan.bundles) {
  await bundle({
    out: plan.out,
    area: area.area,
    contents: entry(area.islands.map((island) => [island.tag, island.file]), "runtime.js", "define"),
    sourcefile: `${area.area}-islands.js`,
    development,
  });
}
