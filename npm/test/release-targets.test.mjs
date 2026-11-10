// The drift gate for what this repository declares it releases: `release-targets.toml`
// reconciled, both ways, with the release configuration it describes.
//
// Nothing here is transcribed. The published set is read from the configuration itself:
// the crate's name from crates/onefollowup/Cargo.toml, the PyPI project from
// pyproject.toml, the npm launcher and its carriers from npm/onefollowup-cli/package.json,
// the platforms from .github/workflows/release.yml's build jobs, scripts/release.sh's
// mapping of each job to its target, scripts/npm-build.mjs's TARGETS, the launcher's
// PACKAGES and rust-toolchain.toml's targets. The secrets the workflows read are held to
// gh-secrets.json, and no workflow gates on a repository variable.
//
// The last cases run the same reconciliation over a declaration changed one way and then
// the other, so the gate is shown catching what it is for.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const read = (path) => readFileSync(join(ROOT, path), "utf8");

/// The first `key = "..."` of a TOML `[section]`.
function tomlValue(toml, section, key) {
  const start = toml.indexOf(`[${section}]`);
  assert.notEqual(start, -1, `no [${section}] section`);
  const rest = toml.slice(start + section.length + 2);
  const end = rest.search(/\n\[/);
  const body = end === -1 ? rest : rest.slice(0, end);
  const found = body.match(new RegExp(`^\\s*${key}\\s*=\\s*"([^"]+)"`, "m"));
  assert.ok(found, `no ${key} in [${section}]`);
  return found[1];
}

/// The declaration's targets: each `[[target]]`'s id, manifest and covers.
function declared(toml) {
  return toml
    .split(/^\[\[target\]\]$/m)
    .slice(1)
    .map((block) => ({
      id: block.match(/^id = "([^"]+)"/m)?.[1],
      manifest: block.match(/^manifest = "([^"]+)"/m)?.[1],
      covers: [...(block.match(/^covers = \[([^\]]*)\]/m)?.[1] ?? "").matchAll(/"([^"]+)"/g)].map(
        (one) => one[1],
      ),
    }));
}

/// Every job of a workflow, in order.
function jobs(workflow) {
  const body = workflow.slice(workflow.indexOf("\njobs:\n"));
  return [...body.matchAll(/^ {2}([a-z][a-z0-9-]*):$/gm)].map((one) => one[1]);
}

/// What the release configuration publishes, read from the configuration.
function published() {
  const crate = tomlValue(read("crates/onefollowup/Cargo.toml"), "package", "name");
  const pypi = tomlValue(read("pyproject.toml"), "project", "name");
  const launcher = JSON.parse(read("npm/onefollowup-cli/package.json"));
  return {
    targets: new Map([
      [`crate:${crate}`, "crates/onefollowup/Cargo.toml"],
      [`pypi:${pypi}`, "pyproject.toml"],
      [`npm:${launcher.name}`, "npm/onefollowup-cli/package.json"],
    ]),
    covers: Object.keys(launcher.optionalDependencies)
      .map((name) => `npm:${name}`)
      .sort(),
  };
}

/// The problems of `toml` as a declaration of what is published.
function drift(toml) {
  const problems = [];
  const { targets, covers } = published();
  const found = declared(toml);
  const ids = new Set(found.map((one) => one.id));
  for (const [id, manifest] of targets) {
    const target = found.find((one) => one.id === id);
    if (!target) problems.push(`${id} is published but not declared`);
    else if (target.manifest !== manifest) problems.push(`${id} names manifest ${target.manifest}, not ${manifest}`);
  }
  for (const id of ids) if (!targets.has(id)) problems.push(`${id} is declared but not published`);
  const coveredByNpm = (found.find((one) => one.id.startsWith("npm:"))?.covers ?? []).sort();
  if (JSON.stringify(coveredByNpm) !== JSON.stringify(covers)) {
    problems.push(`the launcher covers ${coveredByNpm.join(", ")}, but publishes ${covers.join(", ")}`);
  }
  return problems;
}

describe("release-targets.toml", () => {
  it("declares exactly what the release configuration publishes", () => {
    assert.deepEqual(drift(read("release-targets.toml")), []);
    const declaration = read("release-targets.toml");
    assert.match(declaration, /^probe = "scripts\/release-probe\.sh"$/m);
    for (const registry of ["crate", "pypi", "npm"]) {
      assert.match(read("scripts/release-probe.sh"), new RegExp(`^\\s*${registry}\\) url=`, "m"));
    }
  });

  it("names the same four platforms everywhere a release builds them", () => {
    const platforms = ["darwin-arm64", "linux-arm64", "linux-x64", "win32-x64"];
    const launcher = JSON.parse(read("npm/onefollowup-cli/package.json"));
    const carriers = Object.keys(launcher.optionalDependencies)
      .map((name) => name.replace("onefollowup-cli-", ""))
      .sort();
    assert.deepEqual(carriers, platforms, "the launcher's carriers");

    const shim = read("npm/onefollowup-cli/bin/onefollowup.js");
    const packages = [...shim.matchAll(/^ {2}"([a-z0-9]+-[a-z0-9]+)": "onefollowup-cli-\1",$/gm)].map((one) => one[1]);
    assert.deepEqual(packages.sort(), platforms, "the launcher's PACKAGES");

    const build = read("scripts/npm-build.mjs");
    const triples = Object.fromEntries(
      [...build.matchAll(/^ {2}"([a-z0-9_-]+)": \{ platform: "([a-z0-9]+)", arch: "([a-z0-9]+)"/gm)].map((one) => [
        one[1],
        `${one[2]}-${one[3]}`,
      ]),
    );
    assert.deepEqual(Object.values(triples).sort(), platforms, "npm-build.mjs's TARGETS");

    const workflow = read(".github/workflows/release.yml");
    const builds = jobs(workflow).filter((job) => job.startsWith("build-"));
    assert.deepEqual(builds.sort(), ["build-linux", "build-linux-arm64", "build-macos", "build-windows"]);
    const release = read("scripts/release.sh");
    const mapped = builds.map((job) => {
      const found = release.match(new RegExp(`^ {4}${job}\\) echo "([a-z0-9_-]+) ([a-z0-9-]+)" ;;$`, "m"));
      assert.ok(found, `release.sh maps ${job} to a target and a platform`);
      assert.equal(triples[found[1]], found[2], `${job}'s target ${found[1]} is ${found[2]}`);
      assert.match(workflow, new RegExp(`target: ${found[1]}`), `release.yml builds ${job} for ${found[1]}`);
      return found[2];
    });
    assert.deepEqual(mapped.sort(), platforms, "release.sh's build jobs");
    assert.match(release, /uv publish .*"\$out"\/wheels\/\*\.whl/, "each build job uploads its wheel to PyPI");

    const toolchain = read("rust-toolchain.toml");
    for (const triple of Object.keys(triples)) assert.match(toolchain, new RegExp(`"${triple}"`), triple);
    assert.equal(jobs(workflow).at(-1), "publish");
    assert.match(workflow, /needs: \[build-linux, build-linux-arm64, build-macos, build-windows\]/);
  });

  it("publishes with exactly the declared secrets and gates on no repository variable", () => {
    const manifest = JSON.parse(read("gh-secrets.json"));
    const names = manifest.secrets.map((one) => one.name).sort();
    assert.deepEqual(names, [
      "CARGO_REGISTRY_TOKEN",
      "CLAUDE_CODE_OAUTH_TOKEN",
      "NPM_TOKEN",
      "OPENAI_API_KEY",
      "PYPI_TOKEN",
      "RELEASE_PLZ_TOKEN",
    ]);
    const used = new Set();
    for (const workflow of ["ci.yml", "release.yml", "release-plz.yml", "rust-cache-warm.yml"]) {
      const text = read(`.github/workflows/${workflow}`);
      assert.doesNotMatch(text, /\$\{\{[^}]*\bvars\./, `${workflow} gates on no repository variable`);
      for (const one of text.matchAll(/secrets\.([A-Z_]+)/g)) used.add(one[1]);
    }
    used.delete("GITHUB_TOKEN");
    assert.deepEqual([...used].sort(), names, "the workflows read exactly the manifest's secrets");
    const release = read(".github/workflows/release.yml");
    for (const secret of ["PYPI_TOKEN", "NPM_TOKEN", "CARGO_REGISTRY_TOKEN"]) {
      assert.match(release, new RegExp(`secrets\\.${secret}`), `release.yml publishes with ${secret}`);
    }
  });

  it("catches a target published without being declared", () => {
    const declaration = read("release-targets.toml");
    const withoutPypi = declaration.replace(/\[\[target\]\]\nid = "pypi:[^]*?(?=\[\[target\]\])/, "");
    assert.notEqual(withoutPypi, declaration);
    assert.deepEqual(drift(withoutPypi), ["pypi:onefollowup-cli is published but not declared"]);
  });

  it("catches a target declared without being published, and a carrier not covered", () => {
    const declaration = read("release-targets.toml");
    const extra = `${declaration}\n[[target]]\nid = "npm:onefollowup-sdk"\nname = "sdk"\nmanifest = "sdk/package.json"\n`;
    assert.deepEqual(drift(extra), ["npm:onefollowup-sdk is declared but not published"]);
    const uncovered = declaration.replace('  "npm:onefollowup-cli-win32-x64",\n', "");
    assert.equal(drift(uncovered).length, 1);
    assert.match(drift(uncovered)[0], /^the launcher covers /);
  });
});
