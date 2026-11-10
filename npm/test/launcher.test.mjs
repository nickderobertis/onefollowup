// The npm launcher's failure path, run as installed: with no carrier for this platform
// beside it (npm skipped optional dependencies), it exits 1 naming the missing package and
// the other ways to install. The success path — launcher and carrier installed together,
// starting the binary — is scripts/package-smoke.sh, this project's e2e target.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { it } from "node:test";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

it("refuses with what to do when the platform's carrier is not installed", () => {
  const work = mkdtempSync(join(tmpdir(), "onefollowup-launcher-"));
  try {
    const installed = join(work, "node_modules", "onefollowup-cli");
    cpSync(join(ROOT, "npm", "onefollowup-cli"), installed, { recursive: true });
    const run = spawnSync(process.execPath, [join(installed, "bin", "onefollowup.js"), "--version"], {
      cwd: work,
      encoding: "utf8",
    });
    assert.equal(run.status, 1);
    assert.equal(run.stdout, "");
    const platform = `${process.platform}-${process.arch}`;
    assert.match(run.stderr, new RegExp(`onefollowup-cli-${platform} is not installed|unsupported platform ${platform}`));
    assert.match(run.stderr, /pip install onefollowup-cli/);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});
