// Runs the checks of .github/workflows/supply-chain.yml locally: four cargo
// vet / cargo deny checks, then the crate release-age check.
//
//   pnpm supply-chain
//   node scripts/supply-chain.mjs --only release-age
//
// cargo-vet and cargo-deny are the pinned versions below. Keep the versions
// and the linux x86_64 digests in step with the workflow and
// .devcontainer/Dockerfile (scripts/supply-chain.test.mjs checks this).
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const CARGO_VET_VERSION = "0.10.3";
export const CARGO_DENY_VERSION = "0.20.2";

// Digests come from each release's published .sha256 asset and were
// recomputed from the downloaded archives. Platforms without a release asset
// are omitted (cargo-vet publishes no linux arm64 build).
export const TOOLS = {
  "cargo-vet": {
    version: CARGO_VET_VERSION,
    subcommand: "vet",
    url: (target) =>
      `https://github.com/mozilla/cargo-vet/releases/download/v${CARGO_VET_VERSION}/cargo-vet-${target}.tar.xz`,
    extractFlag: "-xJf",
    targets: {
      "darwin-arm64": ["aarch64-apple-darwin", "a83f7d6f48a069890743ba61f7637660c8577f263efd64fee47e673632f0e423"],
      "darwin-x64": ["x86_64-apple-darwin", "d06a5078c3f4c589211ce011dc9329a5849c8153a41b6624112e7318d93c0b6d"],
      "linux-x64": ["x86_64-unknown-linux-gnu", "e83878b80913fbe43629c28768e14ab51f4e86a4ba8f9dd08cc25b8dbc503c19"],
    },
  },
  "cargo-deny": {
    version: CARGO_DENY_VERSION,
    subcommand: "deny",
    url: (target) =>
      `https://github.com/EmbarkStudios/cargo-deny/releases/download/${CARGO_DENY_VERSION}/cargo-deny-${CARGO_DENY_VERSION}-${target}.tar.gz`,
    extractFlag: "-xzf",
    targets: {
      "darwin-arm64": ["aarch64-apple-darwin", "fe67d82a10d8597a3549364cb733a3f9cc1bfff9031b7ae46384a9f2a72090c3"],
      "darwin-x64": ["x86_64-apple-darwin", "248da7f581724e470071990c088ffc55c811981715f4cbdb258621fb79f8b7a6"],
      "linux-x64": ["x86_64-unknown-linux-musl", "9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f"],
      "linux-arm64": ["aarch64-unknown-linux-musl", "995c82be0defc7a025cae49a2aa2644ce8245c9a3318fc4103907c6a285e8c7d"],
    },
  },
};

// Same commands, order, and working directories as the workflow.
export const CHECKS = [
  { name: "cargo vet (root workspace)", args: ["vet", "--locked"], cwd: "." },
  { name: "cargo vet (src-tauri)", args: ["vet", "--locked"], cwd: "src-tauri" },
  { name: "cargo deny (root workspace)", args: ["deny", "check", "advisories", "sources"], cwd: "." },
  {
    name: "cargo deny (src-tauri)",
    args: ["deny", "--manifest-path", "src-tauri/Cargo.toml", "check", "advisories", "sources"],
    cwd: ".",
  },
];

// ---------------------------------------------------------------------------
// Crate release age: no locked crates.io version may be younger than the
// policy's minimum age, unless supply-chain-release-age.json excludes that exact
// version. Mirrors pnpm's minimumReleaseAge (see DEVELOPERS.md).

export const RELEASE_AGE_NAME = "crate release age";
export const RELEASE_AGE_POLICY = "supply-chain-release-age.json";
export const LOCKFILES = ["Cargo.lock", "src-tauri/Cargo.lock"];
export const CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index";
export const USER_AGENT = "threestrands-supply-chain-check";
export const INDEX_CONCURRENCY = 8;
export const API_INTERVAL_MS = 1000; // crates.io crawler policy: 1 request per second
const DAY_MS = 24 * 60 * 60 * 1000;

// Registry packages in a Cargo.lock as [{ name, version }]. Workspace, path,
// and git packages have no `source` or a different one and are skipped.
export function parseLockfile(text) {
  const out = [];
  for (const block of text.split(/^\[\[package\]\]\s*$/m).slice(1)) {
    const field = (key) => block.match(new RegExp(`^${key} = "([^"]*)"`, "m"))?.[1];
    if (field("source") !== CRATES_IO_SOURCE) continue;
    const name = field("name");
    const version = field("version");
    if (name && version) out.push({ name, version });
  }
  return out;
}

// Deduplicated name@version across lockfiles: Map<key, { name, version, lockfiles }>.
export function collectPackages(lockfiles) {
  const packages = new Map();
  for (const [file, text] of Object.entries(lockfiles)) {
    for (const { name, version } of parseLockfile(text)) {
      const key = `${name}@${version}`;
      const entry = packages.get(key) ?? { name, version, lockfiles: [] };
      if (!entry.lockfiles.includes(file)) entry.lockfiles.push(file);
      packages.set(key, entry);
    }
  }
  return packages;
}

export function parsePolicy(text) {
  const policy = JSON.parse(text);
  const days = policy.minimumReleaseAgeDays;
  if (typeof days !== "number" || !Number.isFinite(days) || days < 0) {
    throw new Error(`${RELEASE_AGE_POLICY}: minimumReleaseAgeDays must be a non-negative number`);
  }
  const exclude = policy.exclude ?? [];
  if (!Array.isArray(exclude)) throw new Error(`${RELEASE_AGE_POLICY}: exclude must be an array`);
  for (const e of exclude) {
    for (const key of ["crate", "version", "reason"]) {
      if (typeof e?.[key] !== "string" || !e[key]) {
        throw new Error(`${RELEASE_AGE_POLICY}: every exclusion needs a "${key}" string (exact versions only)`);
      }
    }
  }
  return { minimumReleaseAgeDays: days, exclude };
}

// Sparse-index file location for a crate name.
export function indexPath(name) {
  const n = name.toLowerCase();
  if (n.length <= 2) return `${n.length}/${n}`;
  if (n.length === 3) return `3/${n[0]}/${n}`;
  return `${n.slice(0, 2)}/${n.slice(2, 4)}/${n}`;
}

export function loadPublishCache(file) {
  try {
    const data = JSON.parse(readFileSync(file, "utf8"));
    return data && typeof data === "object" && !Array.isArray(data) ? data : {};
  } catch {
    return {};
  }
}

export function savePublishCache(file, cache) {
  mkdirSync(dirname(file), { recursive: true });
  const tmp = `${file}.${process.pid}.tmp`;
  writeFileSync(tmp, `${JSON.stringify(cache, null, 2)}\n`);
  renameSync(tmp, file);
}

export function publishCachePath(env = process.env) {
  return join(env.XDG_CACHE_HOME || join(homedir(), ".cache"), "threestrands", "crate-publish-times.json");
}

async function getText(fetchFn, url) {
  let lastError;
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const response = await fetchFn(url, { headers: { "User-Agent": USER_AGENT } });
      if (response.ok) return await response.text();
      lastError = new Error(`HTTP ${response.status}`);
      if (response.status < 500) break; // retry only server errors
    } catch (error) {
      lastError = error;
    }
  }
  throw new Error(`${url}: ${lastError.message}`);
}

async function mapLimit(items, limit, fn) {
  const queue = [...items];
  await Promise.all(
    Array.from({ length: Math.min(limit, queue.length) }, async () => {
      while (queue.length) await fn(queue.shift());
    }),
  );
}

// Publish times (ISO strings) for every package, as a Map keyed name@version.
// Cached values are reused. Missing ones come from the sparse index's `pubtime`
// field (one request per crate, INDEX_CONCURRENCY at a time) and, for versions
// without it, the crates.io API at no more than one request per API_INTERVAL_MS.
// Any failure throws; nothing is assumed.
export async function resolvePublishTimes(packages, { fetch: fetchFn, cache, sleep, stats = {} }) {
  stats.cached = 0;
  stats.index = 0;
  stats.api = 0;
  const times = new Map();
  const missing = new Map(); // crate name -> [package]
  for (const [key, pkg] of packages) {
    if (typeof cache[key] === "string") {
      times.set(key, cache[key]);
      stats.cached++;
    } else {
      missing.set(pkg.name, [...(missing.get(pkg.name) ?? []), pkg]);
    }
  }

  const needApi = [];
  await mapLimit([...missing], INDEX_CONCURRENCY, async ([name, pkgs]) => {
    const text = await getText(fetchFn, `https://index.crates.io/${indexPath(name)}`);
    const pubtimes = new Map();
    for (const line of text.split("\n")) {
      if (!line.trim()) continue;
      let entry;
      try {
        entry = JSON.parse(line);
      } catch {
        throw new Error(`index entry for ${name} is not valid JSON`);
      }
      if (typeof entry.pubtime === "string") pubtimes.set(entry.vers, entry.pubtime);
    }
    for (const pkg of pkgs) {
      const key = `${pkg.name}@${pkg.version}`;
      if (pubtimes.has(pkg.version)) {
        times.set(key, pubtimes.get(pkg.version));
        cache[key] = pubtimes.get(pkg.version);
        stats.index++;
      } else {
        needApi.push(pkg);
      }
    }
  });

  for (let i = 0; i < needApi.length; i++) {
    const { name, version } = needApi[i];
    if (i > 0) await sleep(API_INTERVAL_MS);
    const url = `https://crates.io/api/v1/crates/${encodeURIComponent(name)}/${encodeURIComponent(version)}`;
    const created = JSON.parse(await getText(fetchFn, url))?.version?.created_at;
    if (typeof created !== "string") throw new Error(`${url}: response has no version.created_at`);
    times.set(`${name}@${version}`, created);
    cache[`${name}@${version}`] = created;
    stats.api++;
  }
  return times;
}

// Pure decision. A version exactly minimumReleaseAgeDays old passes.
export function evaluateReleaseAge({ packages, times, policy, now }) {
  const limitMs = policy.minimumReleaseAgeDays * DAY_MS;
  const nowMs = now.getTime();
  const offenders = [];
  const warnings = [];
  const excluded = new Set(policy.exclude.map((e) => `${e.crate}@${e.version}`));
  for (const [key, pkg] of packages) {
    const published = new Date(times.get(key));
    const ageMs = nowMs - published.getTime();
    if (ageMs >= limitMs || excluded.has(key)) continue;
    offenders.push({ ...pkg, published, ageMs, eligible: new Date(published.getTime() + limitMs) });
  }
  for (const e of policy.exclude) {
    const key = `${e.crate}@${e.version}`;
    if (!packages.has(key)) {
      warnings.push(`exclusion ${key} is stale: that version is not locked anywhere. Remove it from ${RELEASE_AGE_POLICY}.`);
    } else if (nowMs - new Date(times.get(key)).getTime() >= limitMs) {
      warnings.push(`exclusion ${key} is stale: it is now older than ${policy.minimumReleaseAgeDays} days. Remove it from ${RELEASE_AGE_POLICY}.`);
    }
  }
  return { offenders, warnings };
}

const day = (d) => d.toISOString().slice(0, 10);
const formatAge = (ms) => {
  const hours = Math.floor(ms / (60 * 60 * 1000));
  return hours >= 24 ? `${Math.floor(hours / 24)}d ${hours % 24}h` : `${hours}h`;
};

// Runs the whole check; returns true when it passes. Everything external is
// injected so tests need no network, clock, or home directory.
export async function runReleaseAge({
  repo,
  fetch: fetchFn = globalThis.fetch,
  now = () => new Date(),
  cacheFile = publishCachePath(),
  sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  log = (line) => console.error(line),
} = {}) {
  try {
    const policy = parsePolicy(readFileSync(join(repo, RELEASE_AGE_POLICY), "utf8"));
    const lockfiles = Object.fromEntries(LOCKFILES.map((f) => [f, readFileSync(join(repo, f), "utf8")]));
    const packages = collectPackages(lockfiles);
    const cache = loadPublishCache(cacheFile);
    const stats = {};
    try {
      var times = await resolvePublishTimes(packages, { fetch: fetchFn, cache, sleep, stats });
    } finally {
      savePublishCache(cacheFile, cache); // keep whatever was learned, even on failure
    }
    log(`${packages.size} crates.io versions checked (${stats.cached} cached, ${stats.index} from the index, ${stats.api} from the API)`);
    const { offenders, warnings } = evaluateReleaseAge({ packages, times, policy, now: now() });
    for (const w of warnings) log(`warning: ${w}`);
    for (const o of offenders.sort((a, b) => a.eligible - b.eligible)) {
      log(
        `  ${o.name} ${o.version}: published ${day(o.published)}, ${formatAge(o.ageMs)} old, in ${o.lockfiles.join(" and ")}; ` +
          `eligible ${day(o.eligible)} (${o.eligible.toISOString()})`,
      );
    }
    if (offenders.length) {
      log(`${offenders.length} locked version(s) are younger than ${policy.minimumReleaseAgeDays} days. Wait, or ask the developer about an exclusion in ${RELEASE_AGE_POLICY}.`);
    }
    return offenders.length === 0;
  } catch (error) {
    log(`${RELEASE_AGE_NAME}: ${error.message}`);
    return false;
  }
}

export function assetFor(tool, platform = process.platform, arch = process.arch) {
  const spec = TOOLS[tool];
  const entry = spec.targets[`${platform}-${arch}`];
  if (!entry) {
    throw new Error(
      `No pinned ${tool} ${spec.version} release for ${platform}-${arch}. ` +
        `Install cargo-vet ${CARGO_VET_VERSION} and cargo-deny ${CARGO_DENY_VERSION} yourself ` +
        `(cargo install --locked cargo-vet@${CARGO_VET_VERSION} cargo-deny@${CARGO_DENY_VERSION}) and put them on PATH.`,
    );
  }
  const [target, sha256] = entry;
  return { target, sha256, url: spec.url(target) };
}

export function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

export function verifyDigest(path, expected) {
  const actual = sha256File(path);
  if (actual !== expected) {
    throw new Error(`SHA-256 mismatch for ${path}: expected ${expected}, got ${actual}. Refusing to use it.`);
  }
}

export function cacheRoot(env = process.env) {
  return join(env.XDG_CACHE_HOME || join(homedir(), ".cache"), "threestrands", "supply-chain-tools");
}

export function pathVersionMatches(tool, env = process.env) {
  const spec = TOOLS[tool];
  const result = spawnSync("cargo", [spec.subcommand, "--version"], { encoding: "utf8", env });
  if (result.status !== 0) return false;
  return new RegExp(`(^|\\s)${spec.version.replaceAll(".", "\\.")}(\\s|$)`).test(result.stdout.trim());
}

// Downloads, verifies the digest, then extracts. Returns the directory that
// holds the binary. Reuses an earlier extraction.
export function installTool(tool, { platform, arch, root = cacheRoot(), download = curlDownload } = {}) {
  const spec = TOOLS[tool];
  const { target, sha256, url } = assetFor(tool, platform, arch);
  const dir = join(root, `${tool}-${spec.version}-${target}`);
  if (existsSync(join(dir, tool))) return dir;

  mkdirSync(root, { recursive: true });
  const work = mkdtempSync(join(root, ".download-"));
  try {
    const archive = join(work, "archive");
    download(url, archive);
    verifyDigest(archive, sha256);
    const out = join(work, "out");
    mkdirSync(out);
    run("tar", [spec.extractFlag, archive, "--strip-components=1", "-C", out]);
    if (!existsSync(join(out, tool))) throw new Error(`${tool} binary missing from ${url}`);
    rmSync(dir, { recursive: true, force: true });
    renameSync(out, dir);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
  return dir;
}

function curlDownload(url, dest) {
  console.error(`Downloading ${url}`);
  run("curl", ["--proto", "=https", "--tlsv1.2", "-fsSL", "-o", dest, url]);
}

function run(command, args) {
  const result = spawnSync(command, args, { stdio: ["ignore", "inherit", "inherit"] });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} failed with status ${result.status}`);
}

export async function main(argv = process.argv.slice(2)) {
  const repo = fileURLToPath(new URL("..", import.meta.url));
  const only = argv.indexOf("--only") >= 0 ? argv[argv.indexOf("--only") + 1] : undefined;
  if (only !== undefined && only !== "release-age") {
    console.error(`Unknown --only value ${only}. Supported: release-age`);
    return 2;
  }
  const results = [];
  if (only === undefined) {
    const extraPath = [];
    for (const tool of Object.keys(TOOLS)) {
      if (pathVersionMatches(tool)) continue;
      try {
        extraPath.push(installTool(tool));
      } catch (error) {
        console.error(`${tool}: ${error.message}`);
        return 1;
      }
    }
    const env = { ...process.env, PATH: [...extraPath, process.env.PATH].join(":") };

    for (const check of CHECKS) {
      console.error(`\n==> ${check.name}: cargo ${check.args.join(" ")}`);
      const result = spawnSync("cargo", check.args, { cwd: join(repo, check.cwd), env, stdio: "inherit" });
      results.push({ name: check.name, ok: result.status === 0 });
    }
  }

  console.error(`\n==> ${RELEASE_AGE_NAME}: ${RELEASE_AGE_POLICY}`);
  results.push({ name: RELEASE_AGE_NAME, ok: await runReleaseAge({ repo }) });

  console.error("\nSupply chain summary");
  for (const { name, ok } of results) console.error(`  ${ok ? "PASS" : "FAIL"}  ${name}`);
  return results.every((r) => r.ok) ? 0 : 1;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) process.exitCode = await main();
