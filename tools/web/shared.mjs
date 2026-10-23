// What the island build and the client build share: Svelte compiled with its styles external,
// and every bundle written with `.br` and `.gz` beside it so a server never compresses one per
// request.
import { readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { brotliCompressSync, constants, gzipSync } from "node:zlib";

import * as esbuild from "esbuild";
import { compile } from "svelte/compiler";

const toolchain = dirname(fileURLToPath(import.meta.url));

export async function readPlan() {
  const plan = JSON.parse(await readFile(process.argv[2], "utf8"));
  return { plan, development: plan.mode === "development" };
}

// A module importing each `[key, file]` and handing them, by key, to `call` from `module`.
export function entry(files, module, call) {
  const imports = files.map(([, file], index) => `import C${index} from ${JSON.stringify(file)};`);
  const keyed = files.map(([key], index) => `${JSON.stringify(key)}: C${index}`);
  return `${imports.join("\n")}
import { ${call} } from ${JSON.stringify(join(toolchain, module))};
${call}({ ${keyed.join(", ")} });
`;
}

// `<area>.js` and `<area>.css` in `out`, each with its compressed copies.
export async function bundle({ out, area, contents, sourcefile, development, options = {} }) {
  const outfile = join(out, `${area}.js`);
  await esbuild.build({
    stdin: { contents, resolveDir: toolchain, sourcefile },
    bundle: true,
    outfile,
    format: "esm",
    target: "es2022",
    minify: !development,
    sourcemap: development ? "inline" : false,
    legalComments: "none",
    logLevel: "warning",
    // Islands and screens import `svelte` from wherever they live; it resolves here.
    nodePaths: [join(toolchain, "node_modules")],
    conditions: ["svelte", "browser", development ? "development" : "production"],
    ...options,
    plugins: [svelte(development), ...(options.plugins ?? [])],
  });
  const stylesheet = join(out, `${area}.css`);
  await ensureStylesheet(stylesheet);
  await precompress(outfile, development);
  await precompress(stylesheet, development);
}

// The server's currency table, so amounts are formatted with the same digits and symbols.
export function currencies(table) {
  return {
    name: "sofie-currencies",
    setup(build) {
      build.onResolve({ filter: /^sofie:currencies$/ }, () => ({ path: "currencies", namespace: "sofie" }));
      build.onLoad({ filter: /.*/, namespace: "sofie" }, () => ({
        contents: JSON.stringify(table),
        loader: "json",
      }));
    },
  };
}

async function precompress(file, development) {
  const bytes = await readFile(file);
  const brotli = brotliCompressSync(bytes, {
    params: {
      [constants.BROTLI_PARAM_QUALITY]: development ? 4 : constants.BROTLI_MAX_QUALITY,
      [constants.BROTLI_PARAM_SIZE_HINT]: bytes.length,
    },
  });
  await writeFile(`${file}.br`, brotli);
  await writeFile(`${file}.gz`, gzipSync(bytes, { level: development ? 5 : 9 }));
}

// An area whose components have no styles still answers its stylesheet.
async function ensureStylesheet(file) {
  await readFile(file).catch(() => writeFile(file, ""));
}

// Styles are external, so no inline `<style>` meets the page's Content-Security-Policy: each
// component's CSS is imported back as a module and bundled into the area's stylesheet.
function svelte(development) {
  const styles = new Map();
  return {
    name: "sofie-svelte",
    setup(build) {
      build.onLoad({ filter: /\.svelte$/ }, async ({ path }) => {
        const source = await readFile(path, "utf8");
        let compiled;
        try {
          compiled = compile(source, {
            filename: path,
            css: "external",
            dev: development,
            generate: "client",
          });
        } catch (error) {
          return { errors: [{ text: error.message, location: locate(path, error.start) }] };
        }
        let contents = compiled.js.code;
        if (compiled.css?.code) {
          const style = `${path}.css`;
          styles.set(style, compiled.css.code);
          contents += `\nimport ${JSON.stringify(style)};`;
        }
        const warnings = compiled.warnings.map((warning) => ({
          text: warning.message,
          location: locate(path, warning.start),
        }));
        return { contents, loader: "js", warnings, resolveDir: dirname(path) };
      });
      build.onResolve({ filter: /\.svelte\.css$/ }, ({ path }) => ({ path, namespace: "sofie-svelte-css" }));
      build.onLoad({ filter: /.*/, namespace: "sofie-svelte-css" }, ({ path }) => ({
        contents: styles.get(path),
        loader: "css",
      }));
    },
  };
}

// Where a Svelte diagnostic points, as esbuild reports it: the component's own file.
function locate(file, start) {
  return { file, line: start?.line ?? 1, column: start?.column ?? 0 };
}
