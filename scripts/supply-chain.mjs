// Runs the four checks of .github/workflows/supply-chain.yml locally.
//
//   pnpm supply-chain
//
// cargo-vet and cargo-deny are the pinned versions below. Keep the versions
// and the linux x86_64 digests in step with the workflow and
// .devcontainer/Dockerfile (scripts/supply-chain.test.mjs checks this).
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
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

export function main() {
  const repo = fileURLToPath(new URL("..", import.meta.url));
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

  const results = [];
  for (const check of CHECKS) {
    console.error(`\n==> ${check.name}: cargo ${check.args.join(" ")}`);
    const result = spawnSync("cargo", check.args, { cwd: join(repo, check.cwd), env, stdio: "inherit" });
    results.push({ name: check.name, ok: result.status === 0 });
  }

  console.error("\nSupply chain summary");
  for (const { name, ok } of results) console.error(`  ${ok ? "PASS" : "FAIL"}  ${name}`);
  return results.every((r) => r.ok) ? 0 : 1;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) process.exitCode = main();
