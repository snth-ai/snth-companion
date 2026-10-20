// Builds the face lab into ONE self-contained HTML file: every byte of JS
// and CSS inlined, no external requests, so it opens from file:// and in a
// sandboxed iframe. Uses the esbuild that vite already brings.
//
//   node scripts/build-face-lab.mjs [out.html]     (default /tmp/face-lab/index.html)
//
// The output is not committed; this script is.

import { build } from "esbuild";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const app = resolve(here, "..");
const out = resolve(process.argv[2] ?? "/tmp/face-lab/index.html");

const result = await build({
  entryPoints: [resolve(app, "src/island/face/lab.tsx")],
  bundle: true,
  format: "iife",
  platform: "browser",
  target: ["es2021", "safari15"],
  jsx: "automatic",
  minify: true,
  legalComments: "none",
  define: { "process.env.NODE_ENV": '"production"' },
  loader: { ".json": "json" },
  outdir: resolve(app, "dist-face-lab"),
  write: false,
  logLevel: "warning",
});

const js = result.outputFiles.find((f) => f.path.endsWith(".js"))?.text ?? "";
const css = result.outputFiles.find((f) => f.path.endsWith(".css"))?.text ?? "";
if (!js) throw new Error("esbuild produced no JS");

// Inline safely: nothing in the bundle may close the tags it sits in.
const safeJs = js.replace(/<\/script/gi, "<\\/script").replace(/<!--/g, "<\\!--");
const safeCss = css.replace(/<\/style/gi, "<\\/style");

const html = `<!doctype html>
<html lang="ru">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1.0" />
<meta name="color-scheme" content="dark" />
<link rel="icon" href="data:," />
<title>SNTH Connect: лаборатория лица</title>
<style>${safeCss}</style>
</head>
<body>
<div id="root"></div>
<script>${safeJs}</script>
</body>
</html>
`;

mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, html);
console.log(`face lab: ${out} (${(html.length / 1024).toFixed(1)} KB)`);
