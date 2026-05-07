#!/usr/bin/env node
//
// playwright-worker.js — long-lived Node process the Go companion talks
// to over stdin/stdout JSON.
//
// Wire protocol (newline-delimited JSON):
//   request:  {"id":"<rand>","action":"<name>","args":{...}}
//   response: {"id":"<same>","ok":true|false,"result":<any>,"error":"..."}
//
// The worker keeps ONE persistent Chromium context alive so cookies +
// downloads survive across calls. Profile dir lives at SNTH_PW_PROFILE
// (default ~/Library/Application Support/snth-companion/mia-chrome).
// Headed by default — Mia gets her own dock icon and the user can see
// what she's doing. Headless via SNTH_PW_HEADLESS=1.
//
// The action set is intentionally MINIMAL. Anything DOM-shaped (click,
// type, press, ref-resolution) is just `eval` with the same JS the Go
// side already uses for the CDP backend. Snapshot is `eval_bundle`
// because the snapshot extractor is an IIFE that returns a JSON string
// and stashes window.__snth_tree for follow-up ref ops.

const { chromium } = require("playwright");
const path = require("path");
const fs = require("fs");
const readline = require("readline");

const PROFILE = process.env.SNTH_PW_PROFILE || path.join(
  process.env.HOME,
  "Library/Application Support/snth-companion/mia-chrome",
);
const HEADLESS = process.env.SNTH_PW_HEADLESS === "1";
const DOWNLOADS = process.env.SNTH_PW_DOWNLOADS || path.join(
  process.env.HOME,
  "Library/Application Support/snth-companion/downloads",
);

fs.mkdirSync(PROFILE, { recursive: true });
fs.mkdirSync(DOWNLOADS, { recursive: true });

let context = null;
let activePage = null;

async function ensure() {
  if (context) return;
  context = await chromium.launchPersistentContext(PROFILE, {
    headless: HEADLESS,
    viewport: { width: 1280, height: 800 },
    acceptDownloads: true,
    args: ["--disable-blink-features=AutomationControlled"],
  });
  const pages = context.pages();
  activePage = pages.length ? pages[0] : await context.newPage();
  context.on("page", (p) => { activePage = p; });
  context.on("download", async (dl) => {
    const target = path.join(DOWNLOADS, dl.suggestedFilename());
    try { await dl.saveAs(target); } catch {}
  });
}

async function withPage(fn) {
  await ensure();
  if (!activePage || activePage.isClosed()) {
    activePage = await context.newPage();
  }
  return await fn(activePage);
}

const handlers = {
  // Run a JS expression in the active page context. Returns whatever
  // it evaluates to, JSON-stringified by Playwright. Used by Go side
  // for ref-resolution + small probes.
  eval: (args) => withPage(async (page) => {
    const result = await page.evaluate(args.expr);
    return { result: JSON.stringify(result) };
  }),

  // Run an IIFE bundle that already returns a string (typically JSON).
  // Used for snapshot — the embedded dom_tree.js + wrapper.js bundle is
  // an IIFE that computes the tree, stashes it on window.__snth_tree,
  // and returns JSON.stringify(tree). Mirrors Runtime.evaluate on CDP.
  eval_bundle: (args) => withPage(async (page) => {
    const raw = await page.evaluate(args.bundle);
    return { raw: typeof raw === "string" ? raw : JSON.stringify(raw) };
  }),

  // Navigate. domcontentloaded is enough — full load can hang on slow
  // assets that we don't care about.
  goto: (args) => withPage(async (page) => {
    const resp = await page.goto(args.url, {
      waitUntil: "domcontentloaded",
      timeout: 30000,
    });
    return { final_url: page.url(), status: resp ? resp.status() : 0 };
  }),

  // Press a single keyboard key on the focused element.
  press: (args) => withPage(async (page) => {
    await page.keyboard.press(args.key);
    return { ok: true, key: args.key };
  }),

  // Screenshot of the viewport (not full-page — keeps wire size sane).
  screenshot: (args) => withPage(async (page) => {
    const buf = await page.screenshot({
      type: args.format === "jpeg" ? "jpeg" : "png",
      fullPage: false,
    });
    return {
      data_base64: buf.toString("base64"),
      format: args.format === "jpeg" ? "jpeg" : "png",
    };
  }),

  // Block until the page emits the load event.
  wait_load: (args) => withPage(async (page) => {
    await page.waitForLoadState("load", { timeout: args.timeout_ms || 30000 });
    return { ok: true };
  }),

  // Block until URL matches pattern (Playwright's regex/glob/string).
  wait_url: (args) => withPage(async (page) => {
    await page.waitForURL(args.pattern, { timeout: args.timeout_ms || 30000 });
    return { url: page.url() };
  }),

  // List open tabs in this context.
  tabs: async () => {
    await ensure();
    const out = [];
    for (const p of context.pages()) {
      out.push({ url: p.url(), title: await p.title() });
    }
    return { tabs: out };
  },

  // Identity probe.
  version: async () => {
    await ensure();
    return {
      backend: "playwright",
      profile: PROFILE,
      headless: HEADLESS,
      pages: context.pages().length,
    };
  },
};

function send(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

const rl = readline.createInterface({ input: process.stdin });
rl.on("line", async (line) => {
  let req;
  try { req = JSON.parse(line); }
  catch (e) { send({ id: "", ok: false, error: "bad json: " + e.message }); return; }
  const id = req.id || "";
  const fn = handlers[req.action];
  if (!fn) { send({ id, ok: false, error: "unknown action: " + req.action }); return; }
  try {
    const result = await fn(req.args || {});
    send({ id, ok: true, result });
  } catch (e) {
    send({ id, ok: false, error: e.message || String(e) });
  }
});

// Greeting on stdout signals readiness.
send({ id: "ready", ok: true, result: { profile: PROFILE, headless: HEADLESS } });

const shutdown = async () => {
  if (context) await context.close().catch(() => {});
  process.exit(0);
};
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
