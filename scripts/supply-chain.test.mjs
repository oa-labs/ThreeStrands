import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  CARGO_DENY_VERSION,
  CARGO_VET_VERSION,
  CHECKS,
  TOOLS,
  assetFor,
  installTool,
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

test("runs the same four checks as the workflow", () => {
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
