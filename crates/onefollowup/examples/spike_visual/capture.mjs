// Throwaway (spike-visual; never lands): renders each mock text in texts/ as GitHub renders
// an issue body or comment, <details> collapsed, and screenshots it at a phone's width.
//
//   node crates/onefollowup/examples/spike_visual/capture.mjs <out dir>
//
// marked and playwright are read from SPIKE_NODE_MODULES (default: the ai-orchestrator
// checkout's node_modules), so this branch installs nothing.
import { createRequire } from "node:module";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const modules = process.env.SPIKE_NODE_MODULES ?? "/home/whoop/ai-orchestrator/node_modules";
const require = createRequire(join(modules, "noop.js"));
const { marked } = await import(require.resolve("marked"));
const { chromium } = require("playwright");

const here = dirname(fileURLToPath(import.meta.url));
const out = resolve(process.argv[2] ?? "");
if (!process.argv[2]) {
  console.error("usage: capture.mjs <out dir>");
  process.exit(2);
}
mkdirSync(out, { recursive: true });

// Each view: the mock text, how GitHub frames it, and its title as the 2026-11-18 board dump holds it.
const views = [
  { text: "823", kind: "issue", repo: "nickderobertis/onepipeline", number: 823,
    title: "onepipeline: Republication replays old session events into the run journal" },
  { text: "82", kind: "issue", repo: "nickderobertis/oneharness-ui", number: 82,
    title: "oneharness-ui: Nx build default caches the Cargo target directory" },
  { text: "774", kind: "comment", repo: "nickderobertis/onepipeline", number: 774,
    title: "onepipeline: retained dispatch puts unbounded task prose in one argv word" },
];

// A GitHub-like stylesheet: Primer's light colours, the markdown-body rules a ticket uses.
const css = `
:root { color-scheme: light; }
* { box-sizing: border-box; }
body { margin: 0; background: #fff; color: #1f2328;
  font: 14px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans", Helvetica, Arial, sans-serif; }
.page { padding: 16px; }
.tag { display: inline-block; font-size: 12px; font-weight: 600; padding: 0 8px; border-radius: 2em;
  line-height: 22px; color: #fff; margin-bottom: 8px; }
.tag.before { background: #6e7781; } .tag.after { background: #1f883d; }
.repo { color: #59636e; font-size: 12px; }
h1.title { font-size: 20px; font-weight: 400; margin: 2px 0 8px; line-height: 1.3; }
h1.title .num { color: #59636e; }
.state { display: inline-block; background: #1f883d; color: #fff; border-radius: 2em; padding: 0 10px;
  font-size: 12px; font-weight: 500; line-height: 24px; margin-bottom: 12px; }
.box { border: 1px solid #d1d9e0; border-radius: 6px; }
.box-head { background: #f6f8fa; border-bottom: 1px solid #d1d9e0; padding: 8px 12px; color: #59636e;
  font-size: 13px; border-radius: 6px 6px 0 0; }
.box-head b { color: #1f2328; }
.markdown-body { padding: 12px; word-wrap: break-word; overflow-wrap: anywhere; }
.markdown-body > :first-child { margin-top: 0 !important; }
.markdown-body > :last-child { margin-bottom: 0 !important; }
.markdown-body p, .markdown-body ul, .markdown-body ol, .markdown-body pre, .markdown-body details,
.markdown-body blockquote, .markdown-body table { margin: 0 0 16px; }
.markdown-body h1, .markdown-body h2, .markdown-body h3, .markdown-body h4 { margin: 24px 0 16px;
  font-weight: 600; line-height: 1.25; }
.markdown-body h2 { font-size: 1.5em; padding-bottom: .3em; border-bottom: 1px solid #d1d9e0b3; }
.markdown-body h3 { font-size: 1.25em; } .markdown-body h4 { font-size: 1em; }
.markdown-body ul, .markdown-body ol { padding-left: 2em; }
.markdown-body li + li { margin-top: .25em; }
.markdown-body code { font: 85% ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
  background: #818b981f; padding: .2em .4em; border-radius: 6px; }
.markdown-body pre { background: #f6f8fa; border-radius: 6px; padding: 16px; overflow: auto;
  font-size: 85%; line-height: 1.45; }
.markdown-body pre code { background: none; padding: 0; font-size: 100%; white-space: pre; }
.markdown-body a { color: #0969da; text-decoration: none; }
.markdown-body details summary { cursor: pointer; display: list-item; }
.markdown-body details:not([open]) > *:not(summary) { display: none; }
.markdown-body strong { font-weight: 600; }
`;

const render = (md) => marked.parse(md, { gfm: true, breaks: false });

function page(view, which, md) {
  const head = view.kind === "issue"
    ? `<div class="repo">${view.repo}</div>
       <h1 class="title">${view.title} <span class="num">#${view.number}</span></h1>
       <div class="state">Open</div>
       <div class="box"><div class="box-head"><b>onefollowup</b> opened this issue</div>`
    : `<div class="repo">${view.repo} #${view.number} · ${view.title}</div>
       <div style="height:8px"></div>
       <div class="box"><div class="box-head"><b>onefollowup</b> commented</div>`;
  return `<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><style>${css}</style></head>
<body><div class="page"><span class="tag ${which}">${which === "before" ? "Before" : "After"} migration</span>
${head}<div class="markdown-body">${render(md)}</div></div></div></body></html>`;
}

const browser = await chromium.launch();
try {
  const ctx = await browser.newContext({ viewport: { width: 414, height: 100 }, deviceScaleFactor: 2 });
  const tab = await ctx.newPage();
  for (const view of views) {
    for (const which of ["before", "after"]) {
      const md = readFileSync(join(here, "texts", `${view.text}-${which}.md`), "utf8");
      const html = page(view, which, md);
      writeFileSync(join(out, `${view.text}-${which}.html`), html);
      await tab.setContent(html, { waitUntil: "load" });
      await tab.screenshot({ path: join(out, `${view.text}-${which}.png`), fullPage: true });
    }
  }
} finally {
  await browser.close();
}
