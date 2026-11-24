import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  CARGO_DENY_VERSION,
  CARGO_VET_VERSION,
  CHECKS,
  CRATES_IO_SOURCE,
  RELEASE_AGE_POLICY,
  TOOLS,
  assetFor,
  collectPackages,
  evaluateReleaseAge,
  indexPath,
  installTool,
  parseLockfile,
  parsePolicy,
  resolvePublishTimes,
  runReleaseAge,
  verifyDigest,
} from "./supply-chain.mjs";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

test("selects the release asset and digest for each platform", () => {
  const vet = assetFor("cargo-vet", "darwin", "arm64");
  assert.equal(vet.target, "aarch64-apple-darwin");
  assert.equal(vet.sha256, "a83f7d6f48a069890743ba61f7637660c8577f263efd64fee47e673632f0e423");
  assert.equal(
    vet.url,
    "https://github.com/mozilla/cargo-vet/releases/download/v0.10.3/cargo-vet-aarch64-apple-darwin.tar.xz",
  );
  assert.equal(assetFor("cargo-vet", "linux", "x64").target, "x86_64-unknown-linux-gnu");

  const deny = assetFor("cargo-deny", "linux", "x64");
  assert.equal(deny.target, "x86_64-unknown-linux-musl");
  assert.equal(
    deny.url,
    "https://github.com/EmbarkStudios/cargo-deny/releases/download/0.20.2/cargo-deny-0.20.2-x86_64-unknown-linux-musl.tar.gz",
  );
  assert.equal(assetFor("cargo-deny", "linux", "arm64").target, "aarch64-unknown-linux-musl");
  for (const spec of Object.values(TOOLS)) {
    for (const [, digest] of Object.values(spec.targets)) assert.match(digest, /^[0-9a-f]{64}$/);
  }
});

test("names the tools to install on an unsupported platform", () => {
  assert.throws(() => assetFor("cargo-vet", "linux", "arm64"), /cargo-vet 0\.10\.3 and cargo-deny 0\.20\.2/);
  assert.throws(() => assetFor("cargo-deny", "win32", "x64"), /win32-x64/);
});

test("refuses an archive whose digest differs", () => {
  const dir = mkdtempSync(join(tmpdir(), "supply-chain-test-"));
  const file = join(dir, "archive");
  writeFileSync(file, "not the release");
  assert.throws(() => verifyDigest(file, "0".repeat(64)), /SHA-256 mismatch/);

  const root = mkdtempSync(join(tmpdir(), "supply-chain-cache-"));
  assert.throws(
    () => installTool("cargo-deny", { platform: "linux", arch: "x64", root, download: (_, dest) => writeFileSync(dest, "tampered") }),
    /SHA-256 mismatch/,
  );
});

test("runs the same four cargo checks as the workflow", () => {
  assert.deepEqual(
    CHECKS.map((c) => [c.cwd, ...c.args].join(" ")),
    [
      ". vet --locked",
      "src-tauri vet --locked",
      ". deny check advisories sources",
      ". deny --manifest-path src-tauri/Cargo.toml check advisories sources",
    ],
  );
});

test("pins match the workflow and the dev container", () => {
  const workflow = read(".github/workflows/supply-chain.yml");
  const dockerfile = read(".devcontainer/Dockerfile");
  const value = (text, re) => text.match(re)?.[1];

  assert.equal(value(workflow, /CARGO_VET_VERSION: "([^"]+)"/), CARGO_VET_VERSION);
  assert.equal(value(workflow, /CARGO_DENY_VERSION: "([^"]+)"/), CARGO_DENY_VERSION);
  assert.equal(value(dockerfile, /ARG CARGO_VET_VERSION=(\S+)/), CARGO_VET_VERSION);
  assert.equal(value(dockerfile, /ARG CARGO_DENY_VERSION=(\S+)/), CARGO_DENY_VERSION);
  assert.equal(value(workflow, /CARGO_VET_SHA256: (\w+)/), assetFor("cargo-vet", "linux", "x64").sha256);
  assert.equal(value(workflow, /CARGO_DENY_SHA256: (\w+)/), assetFor("cargo-deny", "linux", "x64").sha256);
});

test("the workflow runs the release-age check and watches its policy file", () => {
  const workflow = read(".github/workflows/supply-chain.yml");
  assert.match(workflow, /node scripts\/supply-chain\.mjs --only release-age/);
  assert.match(workflow, new RegExp(`- "${RELEASE_AGE_POLICY}"`));
  assert.doesNotThrow(() => parsePolicy(read(RELEASE_AGE_POLICY)));
});

// ---- crate release age ----------------------------------------------------

const lock = (...pkgs) =>
  pkgs.map(([name, version, source]) => `[[package]]\nname = "${name}"\nversion = "${version}"\n${source ? `source = "${source}"\nchecksum = "abc"\n` : ""}\n`).join("");
const DAY = 24 * 60 * 60 * 1000;
const NOW = new Date("2026-06-10T12:00:00Z");
const ago = (ms) => new Date(NOW.getTime() - ms).toISOString();
const policy = (...exclude) => ({ minimumReleaseAgeDays: 7, exclude });
const pkgs = (...list) => collectPackages({ "Cargo.lock": lock(...list.map(([n, v]) => [n, v, CRATES_IO_SOURCE])) });

test("parses only crates.io registry packages from a lockfile", () => {
  const text = lock(
    ["threestrands", "0.1.0"],
    ["local", "0.2.0"],
    ["serde", "1.0.1", CRATES_IO_SOURCE],
    ["fork", "1.0.0", "git+https://example.com/fork#abc"],
    ["mirror", "1.0.0", "registry+https://example.com/index"],
    ["sparse", "1.0.0", "sparse+https://index.crates.io/"],
  );
  assert.deepEqual(parseLockfile(text), [{ name: "serde", version: "1.0.1" }]);
});

test("dedupes name@version across lockfiles and records where each is locked", () => {
  const a = lock(["serde", "1.0.1", CRATES_IO_SOURCE], ["url", "2.5.8", CRATES_IO_SOURCE]);
  const b = lock(["serde", "1.0.1", CRATES_IO_SOURCE], ["serde", "1.0.2", CRATES_IO_SOURCE]);
  const packages = collectPackages({ "Cargo.lock": a, "src-tauri/Cargo.lock": b });
  assert.deepEqual([...packages.keys()], ["serde@1.0.1", "url@2.5.8", "serde@1.0.2"]);
  assert.deepEqual(packages.get("serde@1.0.1").lockfiles, ["Cargo.lock", "src-tauri/Cargo.lock"]);
  assert.deepEqual(packages.get("serde@1.0.2").lockfiles, ["src-tauri/Cargo.lock"]);
});

test("maps crate names to sparse-index paths", () => {
  assert.equal(indexPath("a"), "1/a");
  assert.equal(indexPath("io"), "2/io");
  assert.equal(indexPath("url"), "3/u/url");
  assert.equal(indexPath("serde"), "se/rd/serde");
  assert.equal(indexPath("Form_UrlEncoded"), "fo/rm/form_urlencoded");
});

test("release age passes at exactly the limit and fails just under it", () => {
  const packages = pkgs(["below", "1.0.0"], ["exact", "1.0.0"], ["above", "1.0.0"]);
  const times = new Map([
    ["below@1.0.0", ago(7 * DAY - 1000)],
    ["exact@1.0.0", ago(7 * DAY)],
    ["above@1.0.0", ago(7 * DAY + 1000)],
  ]);
  const { offenders } = evaluateReleaseAge({ packages, times, policy: policy(), now: NOW });
  assert.deepEqual(offenders.map((o) => o.name), ["below"]);
  assert.equal(offenders[0].eligible.toISOString(), "2026-06-10T12:00:01.000Z");
  assert.deepEqual(offenders[0].lockfiles, ["Cargo.lock"]);
});

test("exclusions match the exact version only and stale ones warn", () => {
  const packages = pkgs(["url", "2.5.9"], ["url", "2.5.10"], ["old", "1.0.0"]);
  const times = new Map([
    ["url@2.5.9", ago(DAY)],
    ["url@2.5.10", ago(DAY)],
    ["old@1.0.0", ago(30 * DAY)],
  ]);
  const exclude = [
    { crate: "url", version: "2.5.9", reason: "r" },
    { crate: "gone", version: "1.0.0", reason: "r" },
    { crate: "old", version: "1.0.0", reason: "r" },
  ];
  const { offenders, warnings } = evaluateReleaseAge({ packages, times, policy: policy(...exclude), now: NOW });
  assert.deepEqual(offenders.map((o) => `${o.name}@${o.version}`), ["url@2.5.10"]);
  assert.equal(warnings.length, 2);
  assert.match(warnings[0], /gone@1\.0\.0.*not locked/);
  assert.match(warnings[1], /old@1\.0\.0.*older than 7 days/);
});

test("rejects a malformed policy", () => {
  assert.throws(() => parsePolicy("{}"), /minimumReleaseAgeDays/);
  assert.throws(() => parsePolicy('{"minimumReleaseAgeDays":7,"exclude":[{"crate":"url"}]}'), /"version"/);
  assert.deepEqual(parsePolicy('{"minimumReleaseAgeDays":7}').exclude, []);
});

const ok = (body) => ({ ok: true, status: 200, text: async () => body });
const fakeFetch = (routes, calls = []) => async (url, init) => {
  calls.push({ url, init });
  const body = routes[url];
  if (body === undefined) return { ok: false, status: 404, text: async () => "" };
  return ok(typeof body === "string" ? body : JSON.stringify(body));
};
const indexLines = (...entries) => entries.map(([vers, pubtime]) => JSON.stringify({ name: "x", vers, ...(pubtime ? { pubtime } : {}) })).join("\n");

test("uses the index pubtime, falls back to the API one per second, and caches", async () => {
  const packages = pkgs(["serde", "1.0.1"], ["serde", "1.0.2"], ["old", "0.1.0"], ["older", "0.1.0"]);
  const calls = [];
  const routes = {
    "https://index.crates.io/se/rd/serde": indexLines(["1.0.1", "2026-01-01T00:00:00Z"], ["1.0.2", "2026-01-02T00:00:00Z"]),
    "https://index.crates.io/3/o/old": indexLines(["0.1.0"]),
    "https://index.crates.io/ol/de/older": indexLines(["0.1.0"]),
    "https://crates.io/api/v1/crates/old/0.1.0": { version: { created_at: "2020-01-01T00:00:00Z" } },
    "https://crates.io/api/v1/crates/older/0.1.0": { version: { created_at: "2019-01-01T00:00:00Z" } },
  };
  const sleeps = [];
  const cache = {};
  const stats = {};
  const times = await resolvePublishTimes(packages, { fetch: fakeFetch(routes, calls), cache, sleep: async (ms) => sleeps.push(ms), stats });
  assert.equal(times.get("serde@1.0.2"), "2026-01-02T00:00:00Z");
  assert.equal(times.get("old@0.1.0"), "2020-01-01T00:00:00Z");
  assert.deepEqual(stats, { cached: 0, index: 2, api: 2 });
  assert.deepEqual(sleeps, [1000]);
  assert.ok(calls.every((c) => c.init.headers["User-Agent"] === "threestrands-supply-chain-check"));
  assert.equal(Object.keys(cache).length, 4);

  const again = [];
  await resolvePublishTimes(packages, { fetch: fakeFetch({}, again), cache, sleep: async () => {}, stats });
  assert.equal(again.length, 0);
  assert.equal(stats.cached, 4);
});

test("limits index fetch concurrency to 8", async () => {
  const list = Array.from({ length: 30 }, (_, i) => [`crate${i}`, "1.0.0"]);
  let active = 0;
  let peak = 0;
  const fetchFn = async () => {
    active++;
    peak = Math.max(peak, active);
    await new Promise((r) => setTimeout(r, 2));
    active--;
    return ok(indexLines(["1.0.0", "2020-01-01T00:00:00Z"]));
  };
  await resolvePublishTimes(pkgs(...list), { fetch: fetchFn, cache: {}, sleep: async () => {} });
  assert.equal(peak, 8);
});

test("a network failure fails the check instead of passing", async () => {
  const repo = mkdtempSync(join(tmpdir(), "release-age-repo-"));
  writeFileSync(join(repo, RELEASE_AGE_POLICY), JSON.stringify(policy()));
  writeFileSync(join(repo, "Cargo.lock"), lock(["serde", "1.0.1", CRATES_IO_SOURCE]));
  mkdirSync(join(repo, "src-tauri"));
  writeFileSync(join(repo, "src-tauri/Cargo.lock"), "");
  const logs = [];
  const run = (fetchFn) =>
    runReleaseAge({ repo, fetch: fetchFn, now: () => NOW, cacheFile: join(repo, "cache.json"), sleep: async () => {}, log: (l) => logs.push(l) });

  assert.equal(await run(async () => { throw new Error("offline"); }), false);
  assert.match(logs.join("\n"), /offline/);
  assert.equal(await run(async () => ({ ok: false, status: 503, text: async () => "" })), false);
  assert.match(logs.join("\n"), /HTTP 503/);
});

test("end to end: reports offenders with dates, warns on stale exclusions, reuses the cache", async () => {
  const repo = mkdtempSync(join(tmpdir(), "release-age-repo-"));
  mkdirSync(join(repo, "src-tauri"));
  writeFileSync(join(repo, "Cargo.lock"), lock(["serde", "1.0.1", CRATES_IO_SOURCE], ["fresh", "1.0.0", CRATES_IO_SOURCE], ["app", "0.1.0"]));
  writeFileSync(join(repo, "src-tauri/Cargo.lock"), lock(["fresh", "1.0.0", CRATES_IO_SOURCE]));
  writeFileSync(join(repo, RELEASE_AGE_POLICY), JSON.stringify(policy({ crate: "serde", version: "1.0.0", reason: "r" })));
  const routes = {
    "https://index.crates.io/se/rd/serde": indexLines(["1.0.1", ago(30 * DAY)]),
    "https://index.crates.io/fr/es/fresh": indexLines(["1.0.0", ago(2 * DAY)]),
  };
  const logs = [];
  const calls = [];
  const cacheFile = join(repo, "cache", "times.json");
  const run = () =>
    runReleaseAge({ repo, fetch: fakeFetch(routes, calls), now: () => NOW, cacheFile, sleep: async () => {}, log: (l) => logs.push(l) });

  assert.equal(await run(), false);
  const out = logs.join("\n");
  assert.match(out, /fresh 1\.0\.0: published 2026-06-08, 2d 0h old, in Cargo\.lock and src-tauri\/Cargo\.lock; eligible 2026-06-15/);
  assert.match(out, /warning: exclusion serde@1\.0\.0 is stale/);
  assert.doesNotMatch(out, /app 0\.1\.0/);
  const firstCalls = calls.length;
  assert.equal(firstCalls, 2);

  assert.equal(await run(), false);
  assert.equal(calls.length, firstCalls, "second run uses the cache");
});
