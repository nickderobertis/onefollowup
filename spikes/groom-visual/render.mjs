// Throwaway mock: render the Markdown views to GitHub-like HTML and screenshot them.
// Usage: node render.mjs <fixtures dir> <out dir>
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";

const require = createRequire(path.join(process.env.MOCK_NODE_MODULES, "x.js"));
const { marked } = require("marked");
const { chromium } = require("playwright");

const [fixtures, out] = process.argv.slice(2);
const read = (name) => readFileSync(path.join(fixtures, name), "utf8");
const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;");

const css = `
body { margin: 0; background: #fff; color: #1f2328; font: 14px/1.5 -apple-system, "Segoe UI", "Noto Sans", Helvetica, Arial, sans-serif; }
.page { width: 1180px; padding: 32px 40px; }
.md h1 { font-size: 26px; border-bottom: 1px solid #d1d9e0; padding-bottom: 6px; margin: 0 0 12px; }
.md h2 { font-size: 20px; border-bottom: 1px solid #d1d9e0; padding-bottom: 4px; margin: 24px 0 10px; }
.md table { border-collapse: collapse; margin: 8px 0 14px; }
.md th, .md td { border: 1px solid #d1d9e0; padding: 5px 11px; vertical-align: top; }
.md th { background: #f6f8fa; font-weight: 600; text-align: left; }
.md tr:nth-child(2n) td { background: #f6f8fa; }
.md a { color: #0969da; text-decoration: none; }
.md code { background: #eff1f3; border-radius: 5px; padding: 1px 5px; font: 12px ui-monospace, SFMono-Regular, Menlo, monospace; }
.md p { margin: 0 0 10px; }
.card { border: 1px solid #d1d9e0; border-radius: 6px; margin: 0 0 22px; width: 880px; }
.head { background: #f6f8fa; border-bottom: 1px solid #d1d9e0; padding: 8px 14px; border-radius: 6px 6px 0 0; color: #59636e; }
.head b { color: #1f2328; }
.card .md { padding: 12px 16px 4px; }
.event { color: #59636e; margin: -12px 0 22px 18px; font-size: 13px; }
.event b { color: #8250df; }
.src { margin: 0 0 30px; width: 880px; }
.src div { color: #59636e; font-size: 12px; margin-bottom: 3px; }
.src pre { margin: 0; background: #f6f8fa; border: 1px dashed #afb8c1; border-radius: 6px; padding: 8px 12px; font: 12px ui-monospace, Menlo, monospace; white-space: pre-wrap; }
.label { font-weight: 600; font-size: 15px; margin: 0 0 8px; }
`;

const page = (body) => `<!doctype html><meta charset="utf-8"><style>${css}</style><div class="page">${body}</div>`;

function comment(title, file, event) {
  const md = read(file);
  const lines = md.trimEnd().split("\n");
  return `<div class="label">${title}</div>
<div class="card"><div class="head"><b>nickderobertis</b> commented 2026-11-21 10:31 UTC</div><div class="md">${marked.parse(md)}</div></div>
<div class="event">${event}</div>
<div class="src"><div>Markdown source, last line (hidden when GitHub renders it):</div><pre>${esc(lines.slice(-1).join("\n"))}</pre></div>`;
}

const views = {
  "groom-report.png": page(`<div class="md">${marked.parse(read("report.md"))}</div>`),
  "groom-comments.png": page(
    comment("On a ticket closed as fixed — onevcs#325", "comment-fixed.md",
      "nickderobertis <b>closed this as completed</b>") +
    comment("On a duplicate — onevcs#437", "comment-duplicate.md",
      "nickderobertis <b>closed this as a duplicate of</b> #447"),
  ),
};

const browser = await chromium.launch();
const tab = await browser.newPage({ viewport: { width: 1260, height: 800 }, deviceScaleFactor: 1 });
for (const [png, html] of Object.entries(views)) {
  const file = path.join(out, png.replace(".png", ".html"));
  writeFileSync(file, html);
  await tab.goto("file://" + file);
  await tab.locator(".page").screenshot({ path: path.join(out, png) });
}
await browser.close();
