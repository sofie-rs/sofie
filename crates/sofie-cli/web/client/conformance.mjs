// The conformance check, run by sofie-cli's slow tests: `node client/conformance.mjs <dir>`.
// Each `<name>.svelte` in the directory, a compiled screen, is rendered on the server with the
// `data` and `view` in `<name>.json`, formatting the cells its `instants` marks; its HTML is printed as JSON lines `{ name, html }`, which
// the test holds to what the engine renders from the same template.
import { readdir, readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import * as esbuild from "esbuild";
import { compile } from "svelte/compiler";

import { currencies } from "../shared.mjs";

const toolchain = dirname(dirname(fileURLToPath(import.meta.url)));
const directory = process.argv[2];
const screens = (await readdir(directory)).filter((file) => file.endsWith(".svelte"));
const table = JSON.parse(await readFile(join(directory, "currencies.json"), "utf8"));

const entry = `
import { render } from "svelte/server";
${screens.map((file, index) => `import S${index} from ${JSON.stringify(join(directory, file))};`).join("\n")}
export const screens = { ${screens.map((file, index) => `${JSON.stringify(file.replace(/\.svelte$/, ""))}: S${index}`).join(", ")} };
export { render };
export { instantFields } from "./client/core/format.js";
`;
const outfile = join(directory, "conformance.mjs");
await esbuild.build({
  stdin: { contents: entry, resolveDir: toolchain, sourcefile: "conformance-entry.js" },
  bundle: true,
  outfile,
  format: "esm",
  platform: "node",
  logLevel: "warning",
  alias: { "sofie:svelte": join(toolchain, "client", "svelte", "index.js") },
  nodePaths: [join(toolchain, "node_modules")],
  conditions: ["svelte", "production"],
  plugins: [serverSvelte(), currencies(table)],
});
const { screens: compiled, render, instantFields } = await import(pathToFileURL(outfile).href);
for (const [name, component] of Object.entries(compiled)) {
  const { data, view, instants } = JSON.parse(await readFile(join(directory, `${name}.json`), "utf8"));
  const props = { data, view: { ...view, instants: instantFields(data, instants) } };
  const { body } = render(component, { props });
  console.log(JSON.stringify({ name, html: body }));
}

function serverSvelte() {
  return {
    name: "sofie-svelte-server",
    setup(build) {
      build.onLoad({ filter: /\.svelte$/ }, async ({ path }) => {
        const source = await readFile(path, "utf8");
        const { js } = compile(source, { filename: path, generate: "server", css: "external" });
        return { contents: js.code, loader: "js", resolveDir: dirname(path) };
      });
    },
  };
}
