import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

test("rejects a Bun archive with a mismatched checksum before extracting it", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "threestrands-bun-test-"));
  t.after(() => rmSync(cache, { recursive: true, force: true }));
  const launcher = fileURLToPath(new URL("./test-frontend.mjs", import.meta.url));
  const stub = encodeURIComponent('globalThis.fetch = async () => new Response("invalid archive");');
  const result = spawnSync(process.execPath, ["--import", `data:text/javascript,${stub}`, launcher], {
    encoding: "utf8",
    env: { ...process.env, XDG_CACHE_HOME: cache, LOCALAPPDATA: cache },
  });

  assert.ifError(result.error);
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stderr, /Checksum mismatch for bun-/);
  assert.deepEqual(readdirSync(cache), []);
});
