// scripts/release-probe.sh, run as a consumer runs it — a subprocess with only PATH and
// HOME — against a loopback registry standing in for crates.io, PyPI and npm (the one
// external party here, which a test cannot ask to misbehave on cue). Each of the three
// answers is driven: a served version, an empty "no release yet" on a 404, and "not
// answered" (exit 3) for everything else, so a released artifact never reads as unreleased.

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createServer } from "node:http";
import { dirname, join, resolve } from "node:path";
import { after, before, describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const PROBE = join(ROOT, "scripts", "release-probe.sh");

const ROUTES = {
  "/crates/onefollowup": [200, { crate: { max_stable_version: "0.2.0", newest_version: "0.3.0-rc.1" } }],
  "/crates/candidate-only": [200, { crate: { max_stable_version: null, newest_version: "0.1.0-rc.1" } }],
  "/pypi/onefollowup-cli/json": [200, { info: { name: "onefollowup-cli", version: "0.2.0" } }],
  "/npm/onefollowup-cli": [200, { name: "onefollowup-cli", "dist-tags": { latest: "0.2.0", next: "0.3.0-rc.1" } }],
  "/npm/unpublished": [200, { name: "unpublished", time: { unpublished: {} } }],
  "/crates/throttled": [429, { errors: [{ detail: "slow down" }] }],
  "/pypi/reshaped/json": [200, { release: "0.2.0" }],
};

let server;
let base;

before(async () => {
  server = createServer((request, response) => {
    const [status, body] = ROUTES[request.url] ?? [404, { message: "Not Found" }];
    response.writeHead(status, { "content-type": "application/json" });
    response.end(JSON.stringify(body));
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  base = `http://127.0.0.1:${server.address().port}`;
});

after(() => server.close());

function probe(args, registry = base) {
  const env = {
    PATH: process.env.PATH,
    HOME: process.env.HOME ?? "",
    RELEASE_PROBE_CRATES_API: `${registry}/crates`,
    RELEASE_PROBE_PYPI_API: `${registry}/pypi`,
    RELEASE_PROBE_NPM_REGISTRY: `${registry}/npm`,
  };
  return new Promise((done) => {
    execFile("bash", [PROBE, ...args], { env, cwd: ROOT }, (error, stdout, stderr) =>
      done({ status: error ? error.code : 0, stdout, stderr }),
    );
  });
}

describe("release-probe.sh", () => {
  it("answers the version each registry serves", async () => {
    assert.deepEqual(await probe(["crate:onefollowup"]), { status: 0, stdout: "0.2.0\n", stderr: "" });
    assert.deepEqual(await probe(["crate:candidate-only"]), { status: 0, stdout: "0.1.0-rc.1\n", stderr: "" });
    assert.deepEqual(await probe(["pypi:onefollowup-cli"]), { status: 0, stdout: "0.2.0\n", stderr: "" });
    assert.deepEqual(await probe(["npm:onefollowup-cli"]), { status: 0, stdout: "0.2.0\n", stderr: "" });
  });

  it("answers nothing, successfully, only when the registry says there is no such artifact", async () => {
    for (const identifier of ["crate:never-published", "pypi:never-published", "npm:never-published"]) {
      assert.deepEqual(await probe([identifier]), { status: 0, stdout: "", stderr: "" }, identifier);
    }
  });

  it("refuses to answer anything it cannot read unambiguously", async () => {
    const cases = [
      [["crate:throttled"], /answered HTTP 429/],
      [["pypi:reshaped"], /no version could be read/],
      [["npm:unpublished"], /no version could be read/],
      [[], /takes exactly one argument/],
      [["onefollowup"], /is not registry-qualified/],
      [["cargo:onefollowup"], /unknown registry 'cargo'/],
      [["npm:@scope/name"], /is not a registry package name/],
    ];
    for (const [args, message] of cases) {
      const answer = await probe(args);
      assert.equal(answer.status, 3, `${args}: ${answer.stderr}`);
      assert.equal(answer.stdout, "", `${args}`);
      assert.match(answer.stderr, message, `${args}`);
    }
  });

  it("refuses to answer when the registry cannot be reached", async () => {
    const closed = createServer();
    await new Promise((done) => closed.listen(0, "127.0.0.1", done));
    const unreachable = `http://127.0.0.1:${closed.address().port}`;
    await new Promise((done) => closed.close(done));
    const answer = await probe(["crate:onefollowup"], unreachable);
    assert.equal(answer.status, 3);
    assert.equal(answer.stdout, "");
    assert.match(answer.stderr, /could not reach .* NOT evidence|NOT evidence that nothing is published/s);
  });
});
