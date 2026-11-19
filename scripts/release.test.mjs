import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash, generateKeyPairSync, randomBytes, sign } from "node:crypto";
import { chmod, mkdtemp, mkdir, readFile, readdir, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { describe, it } from "node:test";
import {
  INTEL_SKIP_LINE, includesIntel, intelNotice, macosBuilds, prepareReleaseAssets, readTagMessage, releaseMetadata, releaseTagMessage,
  setAppVersion, stampReviewed, updaterPublicKey, verifyUpdateSignature,
} from "./release.mjs";

describe("developer evaluator packaging", () => {
  it("requires an opt-in feature so Cargo and Tauri exclude ai-eval from normal builds", () => {
    const result = spawnSync("cargo", ["metadata", "--format-version", "1", "--no-deps", "--offline",
      "--manifest-path", new URL("../src-tauri/Cargo.toml", import.meta.url).pathname], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    const app = JSON.parse(result.stdout).packages.find((pkg) => pkg.name === "threestrands");
    const evaluator = app.targets.find((target) => target.name === "ai-eval");
    assert.deepEqual(evaluator["required-features"], ["ai-eval"]);
    assert.deepEqual(app.features["ai-eval"], []);
    assert.ok(!(app.features.default ?? []).includes("ai-eval"));
    assert.ok(app.targets.some((target) => target.name === "threestrands" && !target["required-features"]));
  });

  it("enables the evaluator feature through the documented developer command", async () => {
    const pkg = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
    assert.match(pkg.scripts["eval:ai"], /--features ai-eval\b/);
    assert.match(pkg.scripts["eval:ai"], /--bin ai-eval --$/);
  });
});

describe("Linux CI container builds", () => {
  for (const [path, job] of [["release.yml", "linux"], ["linux-build.yml", "linux-amd64"]]) {
    it(`${path} selects an OCI-capable builder before building the amd64 devcontainer`, async () => {
      const workflow = await readFile(new URL(`../.github/workflows/${path}`, import.meta.url), "utf8");
      const jobBlock = workflow.split(`\n  ${job}:\n`)[1]?.split(/\n  [\w-]+:\n/)[0];
      assert.ok(jobBlock, `Missing ${job} job`);
      const steps = jobBlock.split(/\n {6}- /).slice(1);
      const buildIndex = steps.findIndex((step) => /uses: devcontainers\/ci@/.test(step));
      assert.ok(buildIndex >= 0, "Missing devcontainer build step");
      const setup = steps.slice(0, buildIndex).findLast((step) => /uses: docker\/setup-buildx-action@/.test(step));
      assert.ok(setup, "OCI export requires Buildx setup before the devcontainer build");
      assert.match(setup, /^ {10}driver: docker-container$/m);
      assert.match(setup, /^ {10}use: true$/m);
      assert.match(steps[buildIndex], /^ {10}platform: linux\/amd64$/m);
    });

    it(`${path} frees runner disk before building the devcontainer`, async () => {
      const workflow = await readFile(new URL(`../.github/workflows/${path}`, import.meta.url), "utf8");
      const jobBlock = workflow.split(`\n  ${job}:\n`)[1]?.split(/\n  [\w-]+:\n/)[0];
      const steps = jobBlock?.split(/\n {6}- /).slice(1) ?? [];
      const cleanupIndex = steps.findIndex((step) => /^ {8}run: \.\/scripts\/free-ci-disk\.sh$/m.test(step));
      const setupIndex = steps.findIndex((step) => /uses: docker\/setup-buildx-action@/.test(step));
      assert.ok(cleanupIndex >= 0, "Missing disk cleanup step");
      assert.ok(cleanupIndex < setupIndex, "Disk cleanup must run before the image is built");
    });

    it(`${path} forwards resource limits into the Linux build container`, async () => {
      const workflow = await readFile(new URL(`../.github/workflows/${path}`, import.meta.url), "utf8");
      const jobBlock = workflow.split(`\n  ${job}:\n`)[1]?.split(/\n  [\w-]+:\n/)[0];
      const buildStep = jobBlock?.split(/\n {6}- /).find((step) => /uses: devcontainers\/ci@/.test(step));
      assert.ok(buildStep, "Missing devcontainer build step");
      for (const [name, value] of [
        ["CARGO_BUILD_JOBS", "2"], ["CARGO_INCREMENTAL", "0"],
        ["CARGO_PROFILE_DEV_DEBUG", "line-tables-only"],
        ["CARGO_PROFILE_TEST_DEBUG", "line-tables-only"],
      ]) {
        assert.match(buildStep, new RegExp(`^ {10}${name}: "${value}"$`, "m"));
        assert.match(buildStep, new RegExp(`^ {12}${name}$`, "m"), `${name} must reach the container`);
      }
    });
  }

  for (const env of [{}, { GITHUB_ACTIONS: "true" }, { GITHUB_ACTIONS: "true", RUNNER_ENVIRONMENT: "self-hosted" }]) {
    it(`refuses to delete runner SDKs outside a GitHub-hosted runner (${JSON.stringify(env)})`, async (t) => {
      const directory = await mkdtemp(join(tmpdir(), "threestrands disk test "));
      t.after(() => rm(directory, { recursive: true, force: true }));
      const log = join(directory, "commands.log");
      for (const command of ["sudo", "df"]) {
        await writeFile(join(directory, command), `#!/bin/sh\necho ${command} >> "${log}"\n`);
        await chmod(join(directory, command), 0o755);
      }
      const { GITHUB_ACTIONS, RUNNER_ENVIRONMENT, ...base } = process.env;
      const result = spawnSync("bash", [new URL("./free-ci-disk.sh", import.meta.url).pathname], {
        encoding: "utf8",
        env: { ...base, ...env, PATH: `${directory}${delimiter}${process.env.PATH}` },
      });
      assert.equal(result.status, 1);
      assert.match(result.stderr, /only runs on GitHub-hosted Actions runners/);
      await assert.rejects(readFile(log), { code: "ENOENT" }, "No command may run before the guard");
    });
  }

  for (const diagnosticsFail of [false, true]) {
    it(`reports resources after a Rust failure and preserves its exit status (diagnostics fail: ${diagnosticsFail})`, async (t) => {
      const directory = await mkdtemp(join(tmpdir(), "threestrands linux test "));
      t.after(() => rm(directory, { recursive: true, force: true }));
      for (const folder of ["scripts", "bin", "src-tauri/target/debug"]) {
        await mkdir(join(directory, folder), { recursive: true });
      }
      await writeFile(join(directory, "scripts/build-linux.sh"), await readFile(new URL("./build-linux.sh", import.meta.url)));
      const log = join(directory, "commands.jsonl");
      const fakeTool = join(directory, "bin/tool");
      await writeFile(fakeTool, `#!${process.execPath}
const { appendFileSync } = require("node:fs");
const { basename } = require("node:path");
const name = basename(process.argv[1]);
const args = process.argv.slice(2);
appendFileSync(process.env.LINUX_BUILD_TEST_LOG, JSON.stringify([name, ...args]) + "\\n");
if (name === "uname") console.log(args[0] === "-s" ? "Linux" : "x86_64");
else if (name === "cargo" && args[0] === "test") { console.error("simulated linker failure"); process.exit(23); }
else if (["df", "free", "du"].includes(name)) {
  console.log("resource diagnostic: " + name);
  if (process.env.LINUX_BUILD_TEST_DIAGNOSTICS_FAIL === "1") process.exit(9);
} else console.log("test tool " + name);
`);
      await chmod(fakeTool, 0o755);
      for (const name of ["uname", "node", "pnpm", "rustc", "cargo", "df", "free", "du"]) {
        await symlink(fakeTool, join(directory, "bin", name));
      }
      const result = spawnSync("bash", ["scripts/build-linux.sh"], {
        cwd: directory, encoding: "utf8",
        env: {
          ...process.env, PATH: `${join(directory, "bin")}${delimiter}${process.env.PATH}`,
          THREESTRANDS_RELEASE_BUILD: "0", THREESTRANDS_LINUX_BUNDLES: "deb,rpm,appimage",
          LINUX_BUILD_TEST_LOG: log, LINUX_BUILD_TEST_DIAGNOSTICS_FAIL: diagnosticsFail ? "1" : "0",
        },
      });
      assert.equal(result.status, 23, result.stderr);
      assert.match(result.stderr, /simulated linker failure/);
      assert.match(result.stderr, /Linux build failed \(exit 23\)/);
      const commands = (await readFile(log, "utf8")).trim().split("\n").map((line) => JSON.parse(line));
      assert.deepEqual(commands.filter(([name, command]) => name === "pnpm" && ["install", "test", "tauri"].includes(command)), [
        ["pnpm", "install", "--frozen-lockfile"], ["pnpm", "test"],
      ]);
      assert.ok(commands.some((command) => JSON.stringify(command) === JSON.stringify(["cargo", "test", "--locked", "--manifest-path", "src-tauri/Cargo.toml"])));
      for (const name of ["df", "free", "du"]) {
        assert.equal(commands.filter(([tool]) => tool === name).length, 2, `${name} must run before the build and on failure`);
      }
    });
  }

  // build-linux.sh relies on bash 4 (mapfile); it only ever runs inside the Linux devcontainer.
  const bashHasMapfile = spawnSync("bash", ["-c", "type mapfile"]).status === 0;
  it("keeps the copied AppImage executable so it can be extract-verified", { skip: !bashHasMapfile && "bash lacks mapfile" }, async (t) => {
    const directory = await mkdtemp(join(tmpdir(), "threestrands linux test "));
    t.after(() => rm(directory, { recursive: true, force: true }));
    for (const folder of ["scripts", "bin"]) {
      await mkdir(join(directory, folder), { recursive: true });
    }
    await writeFile(join(directory, "scripts/build-linux.sh"), await readFile(new URL("./build-linux.sh", import.meta.url)));
    const fakeTool = join(directory, "bin/tool");
    await writeFile(fakeTool, `#!${process.execPath}
const { chmodSync, mkdirSync, writeFileSync } = require("node:fs");
const { basename, join } = require("node:path");
const name = basename(process.argv[1]);
const args = process.argv.slice(2);
if (name === "uname") console.log(args[0] === "-s" ? "Linux" : "x86_64");
else if (name === "pnpm" && args[0] === "tauri" && args[1] === "build") {
  const bundle = join(process.cwd(), "src-tauri/target/release/bundle");
  for (const [folder, file] of [["deb", "app.deb"], ["rpm", "app.rpm"], ["appimage", "app.AppImage"]]) {
    mkdirSync(join(bundle, folder), { recursive: true });
    const path = join(bundle, folder, file);
    writeFileSync(path, folder === "appimage" ? "#!/bin/sh\\nmkdir -p squashfs-root && touch squashfs-root/AppRun\\n" : "package");
    chmodSync(path, folder === "appimage" ? 0o755 : 0o644);
  }
  mkdirSync(join(process.cwd(), "src-tauri/target/release"), { recursive: true });
} else if (name === "dpkg-deb" && args[0] === "-f") console.log("amd64");
else if (name === "rpm" && args[0] === "-qp" && args[1] === "--queryformat") process.stdout.write("x86_64");
else if (name === "file") console.log(args[0] + ": ELF 64-bit LSB executable, x86-64");
else console.log("test tool " + name);
`);
    await chmod(fakeTool, 0o755);
    for (const name of ["uname", "node", "pnpm", "rustc", "cargo", "df", "free", "du", "dpkg-deb", "rpm", "file", "ldd"]) {
      await symlink(fakeTool, join(directory, "bin", name));
    }
    const result = spawnSync("bash", ["scripts/build-linux.sh"], {
      cwd: directory, encoding: "utf8",
      env: {
        ...process.env, PATH: `${join(directory, "bin")}${delimiter}${process.env.PATH}`,
        THREESTRANDS_RELEASE_BUILD: "0", THREESTRANDS_LINUX_BUNDLES: "deb,rpm,appimage",
      },
    });
    assert.equal(result.status, 0, result.stderr);
    const artifacts = join(directory, "artifacts/linux-amd64");
    const mode = async (file) => (await stat(join(artifacts, file))).mode & 0o777;
    assert.equal(await mode("app.AppImage"), 0o755);
    assert.equal(await mode("app.deb"), 0o644);
    assert.equal(await mode("app.rpm"), 0o644);
    assert.match(await readFile(join(artifacts, "SHA256SUMS"), "utf8"), /app\.AppImage/);
  });
});

/** A minisign key pair in the encodings `tauri signer` writes. */
function updaterKeyPair() {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const keyId = randomBytes(8);
  const raw = publicKey.export({ format: "der", type: "spki" }).subarray(12);
  const pubkey = Buffer.from(`untrusted comment: minisign public key\n${Buffer.concat([Buffer.from("Ed"), keyId, raw]).toString("base64")}\n`).toString("base64");
  const signFile = (data, { trustedComment = "timestamp:1791299842\tfile:fixture" } = {}) => {
    const signature = sign(null, createHash("blake2b512").update(data).digest(), privateKey);
    const global = sign(null, Buffer.concat([signature, Buffer.from(trustedComment)]), privateKey);
    return Buffer.from([
      "untrusted comment: signature from tauri secret key",
      Buffer.concat([Buffer.from("ED"), keyId, signature]).toString("base64"),
      `trusted comment: ${trustedComment}`,
      global.toString("base64"), "",
    ].join("\n")).toString("base64");
  };
  return { pubkey, signFile };
}

const releaseKey = updaterKeyPair();

function tauriConfig(version, pubkey = releaseKey.pubkey) {
  return `${JSON.stringify({ productName: "ThreeStrands", version, plugins: { updater: { pubkey, endpoints: ["https://example.invalid/latest.json"] } } }, null, 2)}\n`;
}

function sources(version = "0.56.0") {
  return {
    packageJson: `${JSON.stringify({ name: "threestrands", version, pnpm: { overrides: { version: "1.0.0" } } }, null, 2)}\n`,
    tauriConfig: tauriConfig(version),
    cargoToml: `[package]\nname = "threestrands"\nversion = "${version}"\n\n[dependencies]\ntauri = "2"\n`,
    cargoLock: `version = 4\n\n[[package]]\nname = "other"\nversion = "9.0.0"\n\n[[package]]\nname = "threestrands"\nversion = "${version}"\n`,
  };
}

describe("release version validation", () => {
  it("classifies stable and beta tags, including zero-valued version components", () => {
    assert.deepEqual(releaseMetadata("v0.56.0", sources()), {
      version: "0.56.0", channel: "stable", prerelease: false,
    });
    assert.deepEqual(releaseMetadata("v0.56.0-beta.1", sources("0.56.0-beta.1")), {
      version: "0.56.0-beta.1", channel: "beta", prerelease: true,
    });
    assert.equal(releaseMetadata("v0.0.0-beta.0", sources("0.0.0-beta.0")).channel, "beta");
  });

  it("rejects malformed tags and unsupported prerelease channels", () => {
    for (const tag of [undefined, "0.56.0", "v0.56", "v00.56.0", "v0.056.0", "v0.56.00",
      "v0.56.0-beta", "v0.56.0-beta.01", "v0.56.0-rc.1", "v0.56.0+build.1", "v0.56.0\n", "v$(whoami)"]) {
      assert.throws(() => releaseMetadata(tag, sources()), /Release tags must/);
    }
  });

  it("rejects a tag mismatch in each of the four application version files", () => {
    for (const key of Object.keys(sources())) {
      assert.throws(() => releaseMetadata("v0.56.0", {
        ...sources(), [key]: sources("0.55.2")[key],
      }), /requires 0\.56\.0/);
    }
  });

  it("requires a valid updater public key so every release can be installed as an update", () => {
    for (const pubkey of [null, "", "  ", "bm90IGEga2V5", Buffer.from("untrusted comment: x\nAAAA\n").toString("base64")]) {
      assert.throws(() => releaseMetadata("v0.56.0", { ...sources(), tauriConfig: tauriConfig("0.56.0", pubkey) }), /pubkey/);
    }
    assert.throws(() => releaseMetadata("v0.56.0", { ...sources(), tauriConfig: JSON.stringify({ version: "0.56.0" }) }), /pubkey/);
  });

  it("requires the application package and never uses a dependency version", () => {
    assert.throws(() => releaseMetadata("v0.56.0", {
      ...sources(), cargoToml: sources().cargoToml.replace('name = "threestrands"', 'name = "other"'),
    }), /application package/);
    assert.throws(() => releaseMetadata("v0.56.0", {
      ...sources(), cargoLock: sources().cargoLock.replace('name = "threestrands"', 'name = "other"'),
    }), /application package/);
    assert.throws(() => releaseMetadata("v0.56.0", {
      ...sources(), cargoLock: `${sources().cargoLock}\n[[package]]\nname = "threestrands"\nversion = "0.56.0"\n`,
    }), /application package/);
    assert.throws(() => releaseMetadata("v0.56.0", {
      ...sources(), cargoToml: '[package]\nname = "threestrands"\n[dependencies]\nversion = "0.56.0"\n',
    }), /version missing/);
  });
});

describe("setting the application version", () => {
  it("changes only the application version in each of the four files", () => {
    const updated = setAppVersion(sources("0.56.0"), "0.57.0");
    for (const key of Object.keys(updated)) {
      assert.equal(updated[key], sources("0.57.0")[key], key);
    }
    assert.match(updated.packageJson, / {6}"version": "1\.0\.0"/, "nested version fields stay as they are");
    assert.match(updated.cargoLock, /name = "other"\nversion = "9\.0\.0"/, "dependency versions stay as they are");
  });

  it("refuses a malformed version or a file without exactly one application version", () => {
    assert.throws(() => setAppVersion(sources(), "0.57"), /Invalid application version/);
    assert.throws(() => setAppVersion({ ...sources(), packageJson: "{}\n" }, "0.57.0"), /one application version in package\.json/);
    assert.throws(() => setAppVersion({ ...sources(), cargoLock: sources().cargoLock.replace('name = "threestrands"', 'name = "other"') }, "0.57.0"),
      /one application version in src-tauri\/Cargo\.lock/);
  });
});

describe("stamping help pages reviewed for the release", () => {
  const page = (reviewed) => `---\ntitle: Search\nreviewed: ${reviewed}\n---\n\nreviewed: next is how to mark a page.\n`;

  it("replaces reviewed: next in the front matter with the version, and only there", () => {
    assert.equal(stampReviewed(page("next"), "0.204.0"), page("0.204.0"));
    assert.match(stampReviewed(page("next"), "0.204.0"), /\nreviewed: next is how/);
  });

  it("leaves pages already reviewed for a version, pages without front matter, and betas alone", () => {
    assert.equal(stampReviewed(page("0.190.0"), "0.204.0"), page("0.190.0"));
    assert.equal(stampReviewed("reviewed: next\n", "0.204.0"), "reviewed: next\n");
    assert.equal(stampReviewed(page("next"), "0.204.0-beta.1"), page("next"));
  });
});

const EMPTY_CHANGELOG = "# Changelog\n\n## Unreleased\n\n## 0.55.0 — 2026-10-01\n\n### New\n- Earlier feature.\n";

async function releaseCommand(t, {
  version = "0.55.0", branch = "master", dirty = "", failAt = "", tags = "", changelog = EMPTY_CHANGELOG, changes = {},
} = {}) {
  const directory = await mkdtemp(join(tmpdir(), "threestrands release test "));
  t.after(() => rm(directory, { recursive: true, force: true }));
  for (const folder of ["scripts", "src-tauri", "bin", "site", "site/docs", "changes"]) await mkdir(join(directory, folder));
  const fixture = sources(version);
  for (const [path, content] of [
    ["package.json", fixture.packageJson], ["src-tauri/tauri.conf.json", fixture.tauriConfig],
    ["src-tauri/Cargo.toml", fixture.cargoToml], ["src-tauri/Cargo.lock", fixture.cargoLock],
    ["CHANGELOG.md", changelog], ["site/docs/compose.md", "Choose **Documented Name**."], ["site/index.html", "<p>Home Page Name</p>"],
    ["changes/README.md", "How to add an entry."],
    ["site/docs/search.md", "---\ntitle: Search\nreviewed: next\n---\n\nFind **Home Page Name**.\n"],
    ["site/docs/files.md", "---\ntitle: Files\nreviewed: 0.50.0\n---\n\nFiles.\n"],
    ...Object.entries(changes).map(([name, content]) => [`changes/${name}`, content]),
  ]) await writeFile(join(directory, path), content);
  for (const script of ["release.mjs", "changelog.mjs"]) {
    await writeFile(join(directory, "scripts", script), await readFile(new URL(`./${script}`, import.meta.url)));
  }

  const log = join(directory, "git-commands.jsonl");
  const fakeGit = join(directory, "bin/git");
  await writeFile(fakeGit, `#!/usr/bin/env node
import { appendFileSync } from "node:fs";
const args = process.argv.slice(2);
appendFileSync(process.env.RELEASE_TEST_LOG, JSON.stringify(args) + "\\n");
if (args[0] === "branch") console.log(process.env.RELEASE_TEST_BRANCH);
if (args[0] === "status") console.log(process.env.RELEASE_TEST_DIRTY);
if (args[0] === "tag" && args[1] === "--list") console.log(process.env.RELEASE_TEST_TAGS);
const action = args[0] === "tag" ? (args[1] === "-s" ? "sign-tag" : "read") : args[0] === "commit" ? "commit" : args[0] === "push" ? (args[2] === "master" ? "push-master" : "push-tag") : "read";
if (action === process.env.RELEASE_TEST_FAIL_AT) process.exit(7);
`);
  await chmod(fakeGit, 0o755);
  const packageJson = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
  const [runtime, script, ...args] = packageJson.scripts.release.split(" ");
  assert.equal(runtime, "node");
  return {
    directory,
    run: (extra = [], commandArgs = [script, ...args, ...extra]) => spawnSync(process.execPath, commandArgs, {
      cwd: directory,
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: `${join(directory, "bin")}${delimiter}${process.env.PATH}`,
        RELEASE_TEST_LOG: log,
        RELEASE_TEST_BRANCH: branch,
        RELEASE_TEST_DIRTY: dirty,
        RELEASE_TEST_FAIL_AT: failAt,
        RELEASE_TEST_TAGS: tags,
      },
    }),
    read: (path) => readFile(join(directory, path), "utf8"),
    exists: (path) => stat(join(directory, path)).then(() => true, () => false),
    commands: async () => {
      try {
        return (await readFile(log, "utf8")).trim().split("\n").map((line) => JSON.parse(line));
      } catch (error) {
        if (error.code === "ENOENT") return [];
        throw error;
      }
    },
  };
}

describe("Windows release build", () => {
  it("adds a Windows job that the release job collects, building updater artifacts", async () => {
    const workflow = await readFile(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");
    const jobBlock = workflow.split("\n  windows:\n")[1]?.split(/\n  [\w-]+:\n/)[0];
    assert.ok(jobBlock, "Missing windows job");
    assert.match(jobBlock, /^ {4}runs-on: windows-latest$/m);
    assert.match(jobBlock, /^ {4}needs: validate$/m);
    // The release job must wait for and gather the Windows artifact.
    assert.match(workflow, /^ {4}needs: \[validate, macos, linux, windows\]$/m);
    assert.match(jobBlock, /name: threestrands-release-windows-x86_64/);
    // The NSIS .exe is both installer and update package, so the updater
    // config must be applied for the build to emit the .exe.sig.
    assert.match(jobBlock, /--config src-tauri\/tauri\.updater\.conf\.json/);
    // The updater key signs the update package even when unsigned on disk.
    assert.match(jobBlock, /TAURI_SIGNING_PRIVATE_KEY: \$\{\{ secrets\.TAURI_SIGNING_PRIVATE_KEY \}\}/);
    // The verify step must assert the updater signature exists.
    assert.match(jobBlock, /\*\.exe\.sig/);
  });

  it("gates Authenticode signing on the Azure secrets so a missing key ships unsigned", async () => {
    const workflow = await readFile(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");
    const jobBlock = workflow.split("\n  windows:\n")[1]?.split(/\n  [\w-]+:\n/)[0];
    const signingStep = jobBlock?.split(/\n {6}- /).find((step) => /id: signing/.test(step));
    assert.ok(signingStep, "Missing the signing-preparation step");
    // All four Azure inputs must be required before signing turns on.
    for (const secret of ["AZURE_SIGNING_ENDPOINT", "AZURE_SIGNING_TENANT_ID", "AZURE_SIGNING_CLIENT_ID", "AZURE_SIGNING_CLIENT_SECRET"]) {
      assert.match(signingStep, new RegExp(`secrets\\.${secret}`), `${secret} must feed the signing gate`);
    }
    // Absent secrets must warn and emit an empty config, not fail the build.
    assert.match(signingStep, /building an UNSIGNED installer/);
    // The signed-config fragment carries only the signer, applied on demand.
    const fragment = JSON.parse(await readFile(new URL("../src-tauri/tauri.windows.signed.conf.json", import.meta.url), "utf8"));
    assert.equal(fragment.bundle.windows.signCommand.cmd, "artifact-signing-cli");
    assert.ok(fragment.bundle.windows.signCommand.args.includes("%1"), "signCommand must pass the binary path placeholder");
    assert.deepEqual(Object.keys(fragment.bundle), ["windows"], "the signed fragment must change nothing but signing");
  });
});

describe("optional release platforms", () => {
  it("creates the draft once macOS builds, leaving out whichever optional platform failed", async () => {
    const workflow = await readFile(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");
    const jobBlock = workflow.split("\n  release:\n")[1]?.split(/\n  [\w-]+:\n/)[0];
    assert.ok(jobBlock, "Missing release job");
    assert.match(jobBlock, /^ {4}if: \$\{\{ !cancelled\(\) && needs\.validate\.result == 'success' && needs\.macos\.result == 'success' \}\}$/m);
    assert.match(jobBlock, /needs\.linux\.result != 'success' && 'linux-amd64'/);
    assert.match(jobBlock, /needs\.windows\.result != 'success' && 'windows-x86_64'/);
    assert.match(jobBlock, /Not in this release/);
  });
});

describe("Intel Mac builds", () => {
  const workflow = () => readFile(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");
  const job = (text, name) => text.split(`\n  ${name}:\n`)[1]?.split(/\n  [\w-]+:\n/)[0];

  it("are built unless a line in the release tag, which the workflow reads back, opts out", () => {
    assert.deepEqual(releaseTagMessage("v0.56.0"), ["-m", "ThreeStrands v0.56.0"]);
    assert.deepEqual(releaseTagMessage("v0.56.0", { intel: false }), ["-m", "ThreeStrands v0.56.0", "-m", INTEL_SKIP_LINE]);
    assert.equal(includesIntel(`ThreeStrands v0.56.0\n\n${INTEL_SKIP_LINE}`), false);
    for (const message of ["ThreeStrands v0.56.0", "", undefined, "Intel: skipped later", "Notes: Intel: skipped", "Intel: included"]) {
      assert.equal(includesIntel(message), true, String(message));
    }
  });

  it("leave Apple Silicon alone when skipped, with tests run only there", () => {
    assert.deepEqual(macosBuilds({ intel: false }), [
      { target: "aarch64-apple-darwin", arch: "aarch64", binary_arch: "arm64", tests: true },
    ]);
    const both = macosBuilds({ intel: true });
    assert.deepEqual(both.map((build) => [build.arch, build.tests]), [["aarch64", true], ["x86_64", false]]);
    assert.deepEqual(both[1], { target: "x86_64-apple-darwin", arch: "x86_64", binary_arch: "x86_64", tests: false });
  });

  it("read the annotated tag's message, and treat a lightweight tag as including them", async (t) => {
    const directory = await mkdtemp(join(tmpdir(), "threestrands-tag-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const git = (...args) => {
      const result = spawnSync("git", ["-c", "user.name=Release Test", "-c", "user.email=release@example.invalid", "-c", "tag.gpgSign=false", ...args], { cwd: directory, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
    };
    git("init", "--quiet");
    git("commit", "--quiet", "--allow-empty", "-m", `Release v0.56.0\n\n${INTEL_SKIP_LINE}`);
    git("tag", "v0.56.0", ...releaseTagMessage("v0.56.0", { intel: false }));
    git("tag", "v0.56.1", ...releaseTagMessage("v0.56.1"));
    git("tag", "v0.56.2");
    assert.equal(includesIntel(readTagMessage(directory, "v0.56.0")), false);
    assert.equal(readTagMessage(directory, "v0.56.1"), "ThreeStrands v0.56.1");
    // A lightweight tag points at the commit; its message must not count.
    assert.equal(readTagMessage(directory, "v0.56.2"), "");
    assert.equal(includesIntel(readTagMessage(directory, "v0.56.2")), true);
    assert.throws(() => readTagMessage(directory, "v0.57.0"), /not in this checkout/);
  });

  it("open release notes without them by linking the latest release that has them", () => {
    assert.equal(intelNotice({ lastIntelTag: "v0.210.0", repository: "oa-labs/ThreeStrands" }),
      "> [!NOTE]\n> This release has no installer for Intel Macs. Intel Macs can install [v0.210.0](https://github.com/oa-labs/ThreeStrands/releases/tag/v0.210.0), the latest release that includes them. Intel Macs are offered updates only by releases that include them.\n");
    assert.equal(intelNotice({ repository: "oa-labs/ThreeStrands" }),
      "> [!NOTE]\n> This release has no installer for Intel Macs. Intel Macs are offered updates only by releases that include them.\n");
  });

  it("are wired through the workflow: the validated tag picks the macOS builds, and the release skips Intel otherwise", async () => {
    const text = await workflow();
    const validate = job(text, "validate");
    assert.match(validate, /git fetch --no-tags --force origin "\+refs\/tags\/\$RELEASE_TAG:refs\/tags\/\$RELEASE_TAG"\n {10}node scripts\/release\.mjs validate "\$RELEASE_TAG"/);
    assert.match(validate, /^ {6}macos_builds: \$\{\{ steps\.version\.outputs\.macos_builds \}\}$/m);
    assert.match(validate, /^ {6}intel: \$\{\{ steps\.version\.outputs\.intel \}\}$/m);

    const macos = job(text, "macos");
    assert.match(macos, /^ {8}include: \$\{\{ fromJSON\(needs\.validate\.outputs\.macos_builds\) \}\}$/m);
    assert.doesNotMatch(macos, /target: x86_64-apple-darwin/, "Intel must come only from the validated tag");
    const steps = macos.split(/\n {6}- /);
    for (const name of ["Run JavaScript tests", "Run Rust tests", "Run shared crate tests"]) {
      const step = steps.find((candidate) => candidate.startsWith(`name: ${name}\n`));
      assert.match(step, /^ {8}if: \$\{\{ matrix\.tests \}\}$/m, name);
    }
    assert.doesNotMatch(steps.find((step) => step.startsWith("name: Build, sign")), /matrix\.tests/);

    const release = job(text, "release");
    assert.match(release, /needs\.validate\.outputs\.intel != 'true' && 'macos-x86_64'/);
    assert.match(release, /RELEASE_INTEL: \$\{\{ needs\.validate\.outputs\.intel \}\}/);
    assert.match(release, /export RELEASE_LAST_INTEL_TAG/);
    assert.match(release, /Intel Macs \(left out with --no-intel\)/);
  });

  it("are left out the same way by scripts/release.sh --no-intel", async () => {
    const script = await readFile(new URL("./release.sh", import.meta.url), "utf8");
    assert.match(script, /^mode=release yes=0 dry_run=0 intel=1 choice=\(\)$/m);
    assert.match(script, /^ {4}--no-intel\) intel=0; shift ;;$/m);
    assert.ok(script.includes(`tag_message+=(-m "${INTEL_SKIP_LINE}")`), "release.sh must write the same tag line");
    assert.match(script, /^git tag -s "\$tag" "\$\{tag_message\[@\]\}"$/m);
  });
});

describe("release builds on the developer's Mac", () => {
  const SHA = "a".repeat(64);
  const RUNNER_URL = "https://github.com/actions/runner/releases/download/v2.330.0/actions-runner-osx-arm64-2.330.0.tar.gz";

  // Stand-ins for tart, gh, and ssh log each call and keep just enough state:
  // which VMs exist, and whether the VM still takes SSH passwords.
  async function runner(t, { vms = ["threestrands-release-base"], policy = "all_external_contributors", xcode = "Xcode 26.1", sudo = "passwordless", failAt = "", failOnce = "", key = true } = {}) {
    const directory = await mkdtemp(join(tmpdir(), "threestrands-runner-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const state = join(directory, "state");
    const home = join(directory, "home");
    await mkdir(join(directory, "bin"));
    await mkdir(state);
    await mkdir(join(home, ".ssh"), { recursive: true });
    if (key) await writeFile(join(home, ".ssh", "id_ed25519.pub"), "ssh-ed25519 AAAAhost host\n");
    for (const name of vms) await writeFile(join(state, `vm-${name}`), "");
    const common = `import { appendFileSync, existsSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
const args = process.argv.slice(2);
const state = (name) => process.env.RUNNER_TEST_STATE + "/" + name;
const fail = (step, code = 1) => {
  if (process.env.RUNNER_TEST_FAIL_AT === step) process.exit(code);
  if (process.env.RUNNER_TEST_FAIL_ONCE === step && !existsSync(state("failed-" + step))) { writeFileSync(state("failed-" + step), ""); process.exit(code); }
};
`;
    for (const [tool, behavior] of Object.entries({
      tart: `appendFileSync(process.env.RUNNER_TEST_LOG, JSON.stringify({ tool: "tart", args, input: "" }) + "\\n");
const [verb, name, other] = args;
if (verb === "get" && !existsSync(state("vm-" + name))) process.exit(1);
if (verb === "clone") { fail("clone"); writeFileSync(state("vm-" + other), ""); }
if (verb === "delete") rmSync(state("vm-" + name));
if (verb === "rename") renameSync(state("vm-" + name), state("vm-" + other));
if (verb === "ip") console.log("192.168.64.5");`,
      gh: `appendFileSync(process.env.RUNNER_TEST_LOG, JSON.stringify({ tool: "gh", args, input: "" }) + "\\n");
const path = args.find((arg) => arg.startsWith("repos/"));
if (path.endsWith("fork-pr-contributor-approval")) {
  if (args.includes("PUT")) { fail("approval"); writeFileSync(state("policy"), "all_external_contributors"); }
  else console.log(existsSync(state("policy")) ? readFileSync(state("policy"), "utf8") : process.env.RUNNER_TEST_POLICY);
}
if (path === "repos/actions/runner/releases/latest") console.log("2.330.0 ${SHA} ${RUNNER_URL}");
if (args.includes("POST")) console.log("4242 ENCODED-JIT-CONFIG");`,
      ssh: `const command = args.at(-1);
const password = args.includes("PubkeyAuthentication=no");
const input = command === "true" || command.startsWith("xcodebuild") || command === "sudo -n true" ? "" : readFileSync(0, "utf8");
appendFileSync(process.env.RUNNER_TEST_LOG, JSON.stringify({ tool: "ssh", args, input, password, askpass: process.env.SSH_ASKPASS ? readFileSync(process.env.SSH_ASKPASS, "utf8") : null, askpassPassword: process.env.RUNNER_ASKPASS_PASSWORD ?? null }) + "\\n");
if (password && existsSync(state("keys-only"))) process.exit(255);
if (command.startsWith("xcodebuild")) console.log(process.env.RUNNER_TEST_XCODE + "\\nBuild version 17A1");
if (command === "sudo -n true" && process.env.RUNNER_TEST_SUDO !== "passwordless") process.exit(1);
if (command.includes("tee /etc/ssh/sshd_config.d/")) writeFileSync(state("keys-only"), "");
if (command.includes("curl")) fail("download");
if (input.includes("ENCODED")) { fail("connect", 255); fail("job", 3); }`,
    })) {
      await writeFile(join(directory, "bin", tool), `#!/usr/bin/env node\n${common}${behavior}\n`);
      await chmod(join(directory, "bin", tool), 0o755);
    }
    const log = join(directory, "commands.jsonl");
    return {
      exists: (name) => stat(join(state, `vm-${name}`)).then(() => true, () => false),
      run: (args = ["--jobs", "1"]) => spawnSync("bash", [new URL("./release-runner.sh", import.meta.url).pathname, ...args], {
        encoding: "utf8",
        env: {
          ...process.env, HOME: home, PATH: `${join(directory, "bin")}${delimiter}${process.env.PATH}`, RUNNER_SSH_KEY: "",
          RUNNER_TEST_LOG: log, RUNNER_TEST_STATE: state, RUNNER_TEST_POLICY: policy, RUNNER_TEST_XCODE: xcode,
          RUNNER_TEST_SUDO: sudo, RUNNER_TEST_FAIL_AT: failAt, RUNNER_TEST_FAIL_ONCE: failOnce, RUNNER_RETRY_DELAY: "0",
        },
      }),
      commands: async () => (await readFile(log, "utf8").catch(() => "")).trim().split("\n").filter(Boolean).map((line) => JSON.parse(line)),
    };
  }
  const summary = (commands) => commands.map(({ tool, args }) => [tool, tool === "ssh" ? args.at(-1).split(" ")[0] : args.slice(0, 2).join(" ")]);
  const approvalCheck = ["gh", "api repos/oa-labs/ThreeStrands/actions/permissions/fork-pr-contributor-approval"];

  describe("serving jobs", () => {
    it("runs each job in a fresh clone of the base VM, then deletes the clone", async (t) => {
      const fixture = await runner(t);
      const result = fixture.run();
      assert.equal(result.status, 0, result.stderr);
      const commands = await fixture.commands();
      const vm = commands.find((command) => command.tool === "tart" && command.args[0] === "clone").args[2];
      assert.match(vm, /^threestrands-release-\d{8}-\d{6}-\d+-1$/);
      // `tart run` starts in the background, so it may log before or after `tart ip`.
      const start = commands.find((command) => command.args[0] === "run");
      assert.deepEqual(start.args, ["run", "--no-graphics", vm]);
      assert.deepEqual(summary(commands.filter((command) => command !== start)), [
        ["tart", "get threestrands-release-base"], approvalCheck, ["tart", "clone threestrands-release-base"],
        ["tart", "ip --wait"], ["ssh", "true"], ["gh", "api -X"], ["ssh", "read"],
        ["gh", "api -X"], ["tart", `stop ${vm}`], ["tart", `delete ${vm}`],
      ]);
      assert.equal(await fixture.exists(vm), false);
    });

    it("registers a single-use runner with the release label, passing its credential only on stdin", async (t) => {
      const fixture = await runner(t);
      assert.equal(fixture.run().status, 0);
      const commands = await fixture.commands();
      const register = commands.find((command) => command.tool === "gh" && command.args.includes("POST"));
      assert.ok(register.args.includes("repos/oa-labs/ThreeStrands/actions/runners/generate-jitconfig"));
      for (const label of ["self-hosted", "macOS", "ARM64", "threestrands-release"]) {
        assert.ok(register.args.includes(`labels[]=${label}`), label);
      }
      const job = commands.find((command) => command.tool === "ssh" && command.input);
      assert.equal(job.input, "ENCODED-JIT-CONFIG\n");
      assert.match(job.args.at(-1), /cd ~\/actions-runner && \.\/run\.sh --jitconfig "\$jit"/);
      assert.ok(job.args.includes("admin@192.168.64.5"));
      assert.ok(job.args.includes("BatchMode=yes"), "serving never falls back to a password");
      for (const command of commands) {
        assert.ok(!command.args.join(" ").includes("ENCODED-JIT-CONFIG"), `${command.tool} must not see the credential in its arguments`);
      }
      const remove = commands.findLast((command) => command.tool === "gh");
      assert.deepEqual(remove.args, ["api", "-X", "DELETE", "repos/oa-labs/ThreeStrands/actions/runners/4242"]);
    });

    it("removes the clone and its runner even when the job fails, and stops after three failures in a row", async (t) => {
      const fixture = await runner(t, { failAt: "job" });
      const result = fixture.run();
      assert.equal(result.status, 1);
      assert.match(result.stderr, /the runner on threestrands-release-\S+ stopped with status 3/);
      assert.match(result.stderr, /3 jobs in a row failed to start; stopping/);
      const commands = await fixture.commands();
      const clones = commands.filter((command) => command.args[0] === "clone").map((command) => command.args[2]);
      assert.deepEqual(clones.map((clone) => clone.at(-1)), ["1", "2", "3"]);
      for (const clone of clones) {
        assert.ok(commands.some((command) => command.args.join(" ") === `delete ${clone}`), clone);
        assert.equal(await fixture.exists(clone), false);
      }
      const removed = commands.filter((command) => command.tool === "gh" && command.args.includes("DELETE"));
      assert.equal(removed.length, 3, "each failed job's runner is removed");
      // A job that ran and failed is not started again in the same VM.
      assert.equal(commands.filter((command) => command.tool === "ssh" && command.input.includes("ENCODED")).length, 3);
    });

    it("connects again when the VM doesn't answer in time to start the runner", async (t) => {
      const fixture = await runner(t, { failOnce: "connect" });
      const result = fixture.run();
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stderr, /Could not connect to threestrands-release-\S+ to start the runner; trying again/);
      const commands = await fixture.commands();
      const starts = commands.filter((command) => command.tool === "ssh" && command.input.includes("ENCODED"));
      assert.equal(starts.length, 2);
      assert.equal(commands.filter((command) => command.args[0] === "clone").length, 1, "the same VM and runner are used");
      assert.equal(commands.filter((command) => command.tool === "gh" && command.args.includes("POST")).length, 1);
    });

    it("keeps serving with a fresh VM after a job fails to start", async (t) => {
      const fixture = await runner(t, { failOnce: "clone" });
      const result = fixture.run();
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stderr, /Starting again with a fresh VM in 0 seconds/);
      const clones = (await fixture.commands()).filter((command) => command.args[0] === "clone").map((command) => command.args[2]);
      assert.equal(clones.length, 2);
      assert.deepEqual(clones.map((clone) => clone.at(-1)), ["1", "2"]);
    });

    it("serves the number of jobs asked for, each in its own clone", async (t) => {
      const fixture = await runner(t);
      assert.equal(fixture.run(["serve", "--jobs", "2"]).status, 0);
      const clones = (await fixture.commands()).filter((command) => command.args[0] === "clone").map((command) => command.args[2]);
      assert.equal(clones.length, 2);
      assert.notEqual(clones[0], clones[1]);
    });

    it("refuses while outside contributors' workflows can run without approval", async (t) => {
      const fixture = await runner(t, { policy: "first_time_contributors" });
      const result = fixture.run();
      assert.equal(result.status, 1);
      assert.match(result.stderr, /without approval \(first_time_contributors\); run: .* require-approval/);
      assert.deepEqual(summary(await fixture.commands()), [["tart", "get threestrands-release-base"], approvalCheck]);
    });

    it("refuses a missing base VM or a bad job count before cloning anything", async (t) => {
      const missing = await runner(t, { vms: [] });
      const result = missing.run();
      assert.equal(result.status, 1);
      assert.match(result.stderr, /no base VM named threestrands-release-base; create it with: .* setup/);
      assert.deepEqual(summary(await missing.commands()), [["tart", "get threestrands-release-base"]]);
      const bad = await runner(t);
      assert.match(bad.run(["--jobs", "0"]).stderr, /--jobs needs a positive number/);
      assert.deepEqual(await bad.commands(), []);
    });
  });

  describe("requiring approval", () => {
    it("requires approval for every outside contributor, then confirms it", async (t) => {
      const fixture = await runner(t, { policy: "first_time_contributors" });
      const result = fixture.run(["require-approval"]);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual((await fixture.commands()).map((command) => command.args), [
        ["api", "-X", "PUT", "repos/oa-labs/ThreeStrands/actions/permissions/fork-pr-contributor-approval", "-f", "approval_policy=all_external_contributors"],
        ["api", "repos/oa-labs/ThreeStrands/actions/permissions/fork-pr-contributor-approval", "--jq", ".approval_policy"],
      ]);
    });

    it("explains when gh may not change the setting", async (t) => {
      const fixture = await runner(t, { failAt: "approval" });
      const result = fixture.run(["require-approval"]);
      assert.equal(result.status, 1);
      assert.match(result.stderr, /gh needs admin access to the repository/);
    });
  });

  describe("creating the base VM", () => {
    it("requires approval first, then builds the base from the image with the key, runner, and keys-only SSH", async (t) => {
      const fixture = await runner(t, { vms: [], policy: "first_time_contributors" });
      const result = fixture.run(["setup"]);
      assert.equal(result.status, 0, result.stderr);
      const commands = await fixture.commands();
      const vm = commands.find((command) => command.args[0] === "clone").args[2];
      assert.match(vm, /^threestrands-release-base-setup-\d+$/);
      assert.deepEqual(commands.find((command) => command.args[0] === "clone").args, ["clone", "ghcr.io/cirruslabs/macos-tahoe-xcode:latest", vm]);
      const start = commands.find((command) => command.args[0] === "run");
      assert.deepEqual(summary(commands.filter((command) => command !== start)).map(([tool, what]) => [tool, what.split(" ")[0] === "api" ? what : what.split(" ")[0]]), [
        ["tart", "get"], ["gh", "api -X"], approvalCheck, ["gh", "api repos/actions/runner/releases/latest"],
        ["tart", "clone"], ["tart", "ip"], ["ssh", "true"], ["ssh", "umask"], ["ssh", "true"], ["ssh", "xcodebuild"],
        ["ssh", "set"], ["ssh", "sudo"], ["ssh", "sudo"], ["ssh", "true"], ["tart", "stop"], ["tart", "get"], ["tart", "rename"],
      ]);
      assert.equal(await fixture.exists("threestrands-release-base"), true);
      assert.equal(await fixture.exists(vm), false);

      const ssh = commands.filter((command) => command.tool === "ssh");
      // The image's password is used only to install the key, through askpass.
      const install = ssh.find((command) => command.args.at(-1).startsWith("umask"));
      assert.equal(install.password, true);
      assert.equal(install.input, "ssh-ed25519 AAAAhost host\n");
      assert.equal(install.askpassPassword, "admin");
      assert.ok(!install.args.join(" ").includes("admin "), "the password never appears in arguments");
      assert.ok(ssh.filter((command) => !command.password).every((command) => command.args.includes("BatchMode=yes")));

      const download = ssh.find((command) => command.args.at(-1).startsWith("set -e"));
      assert.ok(download.args.at(-1).includes(`curl -fsSL -o runner.tar.gz '${RUNNER_URL}'`));
      assert.ok(download.args.at(-1).includes(`echo '${SHA}  runner.tar.gz' | shasum -a 256 -c -`));
      const harden = ssh.find((command) => command.args.at(-1).includes("tee /etc/ssh/sshd_config.d/100-threestrands-release.conf"));
      assert.equal(harden.input, "PasswordAuthentication no\nKbdInteractiveAuthentication no\n");
      assert.equal(ssh.at(-1).password, true, "it checks that the password no longer works");
      assert.doesNotMatch(result.stderr, /still accepts the password/);
    });

    it("sends the image's password to sudo ahead of the settings when sudo asks for one", async (t) => {
      const fixture = await runner(t, { vms: [], sudo: "password" });
      assert.equal(fixture.run(["setup"]).status, 0);
      const harden = (await fixture.commands()).find((command) => command.tool === "ssh" && command.args.at(-1).includes("tee /etc/ssh"));
      assert.match(harden.args.at(-1), /^sudo -S -p '' tee /);
      assert.equal(harden.input, "admin\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n");
    });

    it("deletes the half-built VM and keeps an existing base when a step fails", async (t) => {
      for (const [options, message] of [
        [{ xcode: "Xcode 16.4" }, /has Xcode 16\.4; release builds need Xcode 26 or later/],
        [{ failAt: "download" }, /installing the Actions runner failed/],
      ]) {
        const fixture = await runner(t, options);
        const result = fixture.run(["setup", "--replace"]);
        assert.equal(result.status, 1);
        assert.match(result.stderr, message);
        const commands = await fixture.commands();
        const vm = commands.find((command) => command.args[0] === "clone").args[2];
        assert.equal(await fixture.exists(vm), false, "the half-built VM is deleted");
        assert.equal(await fixture.exists("threestrands-release-base"), true, "the existing base is untouched");
        assert.ok(!commands.some((command) => command.args[0] === "rename"));
      }
    });

    it("replaces an existing base only with --replace, and needs an SSH key", async (t) => {
      const existing = await runner(t);
      const refused = existing.run(["setup"]);
      assert.equal(refused.status, 1);
      assert.match(refused.stderr, /already exists; pass --replace/);
      assert.deepEqual(summary(await existing.commands()), [["tart", "get threestrands-release-base"]]);

      const replaced = await runner(t);
      assert.equal(replaced.run(["setup", "--replace", "--image", "example/xcode:26"]).status, 0);
      const commands = await replaced.commands();
      assert.equal(commands.find((command) => command.args[0] === "clone").args[1], "example/xcode:26");
      assert.ok(commands.some((command) => command.tool === "tart" && command.args.join(" ") === "delete threestrands-release-base"));
      assert.equal(await replaced.exists("threestrands-release-base"), true);

      const keyless = await runner(t, { vms: [], key: false });
      assert.match(keyless.run(["setup"]).stderr, /no SSH public key found/);
      assert.ok(!(await keyless.commands()).some((command) => command.args[0] === "clone"));
    });
  });

  it("is chosen by a repository variable, with GitHub's Macs when it is unset", async () => {
    const workflow = await readFile(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");
    const macos = workflow.split("\n  macos:\n")[1]?.split(/\n  [\w-]+:\n/)[0];
    assert.match(macos, /^ {4}runs-on: \$\{\{ vars\.MACOS_RELEASE_RUNNER && fromJSON\(vars\.MACOS_RELEASE_RUNNER\) \|\| 'macos-26' \}\}$/m);
    // Only the macOS builds move; the other jobs stay on GitHub's runners.
    assert.equal(workflow.match(/vars\.MACOS_RELEASE_RUNNER/g)?.length, 2);
  });
});

describe("marketing site refresh", () => {
  it("redeploys the site from master only after a stable release is built", async () => {
    const workflow = await readFile(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");
    const jobBlock = workflow.split("\n  site:\n")[1]?.split(/\n  [\w-]+:\n/)[0];
    assert.ok(jobBlock, "Missing site job");
    assert.match(jobBlock, /^ {4}needs: \[validate, release\]$/m);
    assert.match(jobBlock, /^ {4}if: \$\{\{ !cancelled\(\) && needs\.release\.result == 'success' && needs\.validate\.outputs\.prerelease == 'false' \}\}$/m);
    assert.match(jobBlock, /^ {6}actions: write$/m);
    assert.doesNotMatch(jobBlock, /contents: write|secrets\./, "Site trigger needs no write or signing access");
    assert.match(jobBlock, /run: gh workflow run site\.yml --ref master$/m);

    const site = await readFile(new URL("../.github/workflows/site.yml", import.meta.url), "utf8");
    assert.match(site, /^ {2}workflow_dispatch:$/m, "site.yml must accept the dispatch");
  });
});

describe("release notes", () => {
  it("builds GitHub release notes from CHANGELOG.md, counting from the last published release", async () => {
    const workflow = await readFile(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");
    const jobBlock = workflow.split("\n  release:\n")[1]?.split(/\n  [\w-]+:\n/)[0];
    assert.ok(jobBlock, "Missing release job");
    assert.doesNotMatch(jobBlock, /--generate-notes/, "Commit-title notes replace the changelog");
    assert.match(jobBlock, /gh release list --exclude-drafts .*> "\$RUNNER_TEMP\/published-tags"/);
    assert.match(jobBlock, /node scripts\/release\.mjs notes "\$RELEASE_TAG" "\$RUNNER_TEMP\/published-tags" > "\$RUNNER_TEMP\/release-notes\.md"/);
    const notesFile = /--notes-file "\$RUNNER_TEMP\/release-notes\.md"/g;
    assert.equal(jobBlock.match(notesFile)?.length, 2, "Both new and re-cut draft releases get the notes");
    assert.match(workflow, /node --test scripts\/release\.test\.mjs scripts\/changelog\.test\.mjs/);
  });
});

describe("pnpm release command", () => {
  const VERSION_PATHS = ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock"];
  const DOC = "site/docs/search.md";
  const commands = (version, paths = [...VERSION_PATHS, "CHANGELOG.md"]) => [
    ["branch", "--show-current"], ["status", "--porcelain"], ["tag", "--list", `v${version}`],
    ...(paths.length ? [["add", "-A", "--", ...paths], ["commit", "-m", `Release v${version}`]] : []),
    ["push", "primary", "master"],
    ["tag", "-s", `v${version}`, "-m", `ThreeStrands v${version}`],
    ["push", "primary", `v${version}`],
  ];
  const fix = { "clearer-fix.md": "### Fixed\n- A clearer fix.\n" };
  const feature = { "snooze.md": "### New\n- **Snooze** a conversation.\n" };
  const withoutIntel = (list) => list.map((args) => args[0] === "tag" && args[1] === "-s" ? [...args, "-m", INTEL_SKIP_LINE] : args);

  it("leaves Intel Macs out with pnpm release --no-intel, alone or with a version choice", async (t) => {
    for (const [args, version] of [[["--no-intel"], "0.55.1"], [["--no-intel", "--major"], "1.0.0"], [["0.56.0", "--no-intel"], "0.56.0"]]) {
      const fixture = await releaseCommand(t, { changes: fix });
      const result = fixture.run(args);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /without Intel Macs/);
      assert.deepEqual(await fixture.commands(), withoutIntel(commands(version, [...VERSION_PATHS, "CHANGELOG.md", DOC, "changes/clearer-fix.md"])), args.join(" "));
    }
  });

  async function assertVersion(fixture, version) {
    const written = { packageJson: "package.json", tauriConfig: "src-tauri/tauri.conf.json", cargoToml: "src-tauri/Cargo.toml", cargoLock: "src-tauri/Cargo.lock" };
    for (const [key, path] of Object.entries(written)) assert.equal(await fixture.read(path), sources(version)[key], path);
  }

  it("releases the next patch for fixes: sets all four versions, stamps the entries, and deletes their change files", async (t) => {
    const fixture = await releaseCommand(t, { changes: fix });
    const result = fixture.run();
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(await fixture.commands(), commands("0.55.1", [...VERSION_PATHS, "CHANGELOG.md", DOC, "changes/clearer-fix.md"]));
    await assertVersion(fixture, "0.55.1");
    assert.match(await fixture.read("CHANGELOG.md"), /^## Unreleased\n\n## 0\.55\.1 — \d{4}-\d{2}-\d{2}\n\n### Fixed\n- A clearer fix\.\n\n## 0\.55\.0/m);
    assert.equal(await fixture.exists("changes/clearer-fix.md"), false);
    assert.equal(await fixture.exists("changes/README.md"), true);
    assert.match(await fixture.read(DOC), /^---\ntitle: Search\nreviewed: 0\.55\.1\n---\n/);
    assert.match(await fixture.read("site/docs/files.md"), /reviewed: 0\.50\.0\n/);
  });

  it("releases the next minor when a change file or Unreleased has something New, in file-name order", async (t) => {
    const fixture = await releaseCommand(t, {
      changes: { ...fix, ...feature, "another-fix.md": "### Fixed\n- Another fix.\n" },
      changelog: EMPTY_CHANGELOG.replace("## Unreleased\n", "## Unreleased\n\n### Fixed\n- An older fix.\n"),
    });
    const result = fixture.run();
    assert.equal(result.status, 0, result.stderr);
    await assertVersion(fixture, "0.56.0");
    assert.match(await fixture.read("CHANGELOG.md"),
      /## 0\.56\.0 — \S+\n\n### New\n- \*\*Snooze\*\* a conversation\.\n\n### Fixed\n- An older fix\.\n- Another fix\.\n- A clearer fix\.\n/);

    const inChangelog = await releaseCommand(t, { changelog: EMPTY_CHANGELOG.replace("## Unreleased\n", "## Unreleased\n\n### New\n- Snooze.\n") });
    assert.equal(inChangelog.run().status, 0);
    await assertVersion(inChangelog, "0.56.0");
  });

  it("releases a patch of only the version files when nothing is pending", async (t) => {
    const fixture = await releaseCommand(t);
    const result = fixture.run();
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(await fixture.commands(), commands("0.55.1", [...VERSION_PATHS, DOC]));
    assert.equal(await fixture.read("CHANGELOG.md"), EMPTY_CHANGELOG);
  });

  for (const [args, version] of [[["--major"], "1.0.0"], [["0.56.0-beta.1"], "0.56.0-beta.1"], [["0.60.0"], "0.60.0"]]) {
    it(`releases ${version} when given ${args.join(" ")}`, async (t) => {
      const fixture = await releaseCommand(t, { changes: fix });
      const result = fixture.run(args);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual((await fixture.commands()).at(-1), ["push", "primary", `v${version}`]);
      await assertVersion(fixture, version);
      assert.match(await fixture.read(DOC), version.includes("-beta.") ? /reviewed: next\n/ : new RegExp(`reviewed: ${version.replaceAll(".", "\\.")}\n`));
    });
  }

  it("re-cuts the current version, leaving the version files alone and merging new entries into its section", async (t) => {
    const fixture = await releaseCommand(t, { changes: fix });
    const result = fixture.run(["0.55.0"]);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(await fixture.commands(), commands("0.55.0", ["CHANGELOG.md", DOC, "changes/clearer-fix.md"]));
    await assertVersion(fixture, "0.55.0");
    assert.match(await fixture.read("CHANGELOG.md"), /## 0\.55\.0 — 2026-10-01\n\n### New\n- Earlier feature\.\n\n### Fixed\n- A clearer fix\.\n/);
  });

  for (const [problem, args, message] of [
    ["an older version", ["0.54.9"], /0\.54\.9 is older than the current version 0\.55\.0/],
    ["a malformed version", ["0.56"], /Invalid version 0\.56/],
    ["both a version and --major", ["--major", "1.0.0"], /at most one/],
  ]) {
    it(`refuses ${problem} before invoking Git or changing anything`, async (t) => {
      const fixture = await releaseCommand(t, { changes: fix });
      const result = fixture.run(args);
      assert.equal(result.status, 1);
      assert.match(result.stderr, message);
      assert.deepEqual(await fixture.commands(), []);
      await assertVersion(fixture, "0.55.0");
    });
  }

  for (const [failAt, index] of [["push-master", 5], ["sign-tag", 6], ["push-tag", 7]]) {
    it(`stops when ${failAt} fails`, async (t) => {
      const fixture = await releaseCommand(t, { failAt });
      const result = fixture.run();
      assert.equal(result.status, 1);
      assert.match(result.stderr, /failed \(exit 7\)/);
      assert.deepEqual(await fixture.commands(), commands("0.55.1", [...VERSION_PATHS, DOC]).slice(0, index + 1));
    });
  }

  it("does not push when the release commit fails", async (t) => {
    const fixture = await releaseCommand(t, { changes: fix, failAt: "commit" });
    assert.equal(fixture.run().status, 1);
    assert.deepEqual(await fixture.commands(), commands("0.55.1", [...VERSION_PATHS, "CHANGELOG.md", DOC, "changes/clearer-fix.md"]).slice(0, 5));
  });

  it("requires the master branch, committed changes, and an unused tag before changing anything", async (t) => {
    const branch = await releaseCommand(t, { branch: "feature", changes: fix });
    assert.match(branch.run().stderr, /Switch to master/);
    assert.deepEqual(await branch.commands(), commands("0.55.1").slice(0, 1));
    const dirty = await releaseCommand(t, { dirty: " M package.json", changes: fix });
    assert.match(dirty.run().stderr, /Commit or stash/);
    assert.deepEqual(await dirty.commands(), commands("0.55.1").slice(0, 2));
    const tagged = await releaseCommand(t, { tags: "v0.55.1", changes: fix });
    assert.match(tagged.run().stderr, /Tag v0\.55\.1 already exists/);
    assert.deepEqual(await tagged.commands(), commands("0.55.1").slice(0, 3));
    for (const fixture of [branch, dirty, tagged]) {
      await assertVersion(fixture, "0.55.0");
      assert.equal(await fixture.exists("changes/clearer-fix.md"), true);
    }
  });

  it("warns about bold names in pending entries missing from the site, then releases anyway", async (t) => {
    const fixture = await releaseCommand(t, {
      changes: { "names.md": "### New\n- **Missing Name**, **Documented Name**, and **Home Page Name**.\n" },
    });
    const result = fixture.run();
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /Not mentioned in site\/docs or site\/index\.html: Missing Name\n/);
    assert.deepEqual((await fixture.commands()).at(-1), ["push", "primary", "v0.56.0"]);
  });

  for (const [problem, options, message] of [
    ["a malformed changelog", { changelog: EMPTY_CHANGELOG.replace("### New", "### Changes") }, /CHANGELOG\.md line \d+: groups are New, Improved, Fixed/],
    ["a malformed change file", { changes: { ...fix, "bad.md": "### Fixed\n- Fine.\n\n### Changes\n- x\n" } }, /changes\/bad\.md line 4: groups are/],
  ]) {
    it(`refuses ${problem} before invoking Git or changing anything`, async (t) => {
      const fixture = await releaseCommand(t, options);
      const result = fixture.run();
      assert.equal(result.status, 1);
      assert.match(result.stderr, message);
      assert.deepEqual(await fixture.commands(), []);
      await assertVersion(fixture, "0.55.0");
    });
  }

  it("rejects inconsistent application versions before invoking Git", async (t) => {
    const fixture = await releaseCommand(t);
    await writeFile(join(fixture.directory, "src-tauri/Cargo.lock"), sources("0.55.3").cargoLock);
    const result = fixture.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Cargo\.lock.*requires 0\.55\.0/);
    assert.deepEqual(await fixture.commands(), []);
  });
});

const signedUpdates = [["macos-aarch64", "ThreeStrands.app.tar.gz"], ["macos-x86_64", "ThreeStrands.app.tar.gz"], ["linux-amd64", "app.AppImage"], ["windows-x86_64", "ThreeStrands_setup.exe"]];

async function artifacts(t, { signFile = releaseKey.signFile } = {}) {
  const directory = await mkdtemp(join(tmpdir(), "threestrands-release-test-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const files = [
    ["macos-aarch64", "arm.dmg"], ["macos-x86_64", "intel.dmg"],
    ["linux-amd64", "app.deb"], ["linux-amd64", "app.rpm"], ["linux-amd64", "app.AppImage"],
    ["windows-x86_64", "ThreeStrands_setup.exe"],
    ["macos-aarch64", "ThreeStrands.app.tar.gz"], ["macos-x86_64", "ThreeStrands.app.tar.gz"],
  ];
  for (const [platform, name] of files) {
    const folder = join(directory, `threestrands-release-${platform}`);
    await mkdir(folder, { recursive: true });
    await writeFile(join(folder, name), `fixture ${platform} ${name}`);
  }
  for (const [platform, name] of signedUpdates) {
    const folder = join(directory, `threestrands-release-${platform}`);
    await writeFile(join(folder, `${name}.sig`), signFile(await readFile(join(folder, name))));
  }
  return { directory, output: join(directory, "output") };
}

const prepare = (directory, output, version, key = releaseKey.pubkey, skip = []) => prepareReleaseAssets(directory, output, version, {
  updaterKey: updaterPublicKey(tauriConfig(version, key)), now: new Date("2026-10-06T12:00:00Z"), skip,
});

describe("release installer collection", () => {
  it("collects all installers and update packages with versioned names and verifiable hashes, excluding diagnostics", async (t) => {
    const { directory, output } = await artifacts(t);
    await writeFile(join(directory, "threestrands-release-linux-amd64", "SHA256SUMS"), "old hashes");
    await writeFile(join(directory, "threestrands-release-linux-amd64", "native-library-dependencies.txt"), "diagnostics");
    const names = await prepare(directory, output, "0.56.0-beta.1");
    const packages = [
      "ThreeStrands_0.56.0-beta.1_macos_aarch64.dmg", "ThreeStrands_0.56.0-beta.1_macos_x86_64.dmg",
      "ThreeStrands_0.56.0-beta.1_linux_amd64.deb", "ThreeStrands_0.56.0-beta.1_linux_amd64.rpm",
      "ThreeStrands_0.56.0-beta.1_linux_amd64.AppImage",
      "ThreeStrands_0.56.0-beta.1_windows_x86_64.exe",
      "ThreeStrands_0.56.0-beta.1_macos_aarch64.app.tar.gz", "ThreeStrands_0.56.0-beta.1_macos_x86_64.app.tar.gz",
    ];
    assert.deepEqual(names, [...packages, "latest.json"]);
    assert.deepEqual((await readdir(output)).sort(), [...names, "SHA256SUMS"].sort());
    const expected = [];
    for (const name of packages) {
      const digest = createHash("sha256").update(await readFile(join(output, name))).digest("hex");
      expected.push(`${digest}  ${name}\n`);
    }
    assert.equal(await readFile(join(output, "SHA256SUMS"), "utf8"), expected.join(""));
  });

  it("writes an update manifest pointing each platform at its signed package", async (t) => {
    const { directory, output } = await artifacts(t);
    await prepare(directory, output, "0.56.0");
    const manifest = JSON.parse(await readFile(join(output, "latest.json"), "utf8"));
    const download = "https://github.com/oa-labs/ThreeStrands/releases/download/v0.56.0";
    const signature = async (platform, name) => (await readFile(join(directory, `threestrands-release-${platform}`, `${name}.sig`), "utf8")).trim();
    assert.deepEqual(manifest, {
      version: "0.56.0",
      pub_date: "2026-10-06T12:00:00.000Z",
      platforms: {
        "darwin-aarch64": { signature: await signature("macos-aarch64", "ThreeStrands.app.tar.gz"), url: `${download}/ThreeStrands_0.56.0_macos_aarch64.app.tar.gz` },
        "darwin-x86_64": { signature: await signature("macos-x86_64", "ThreeStrands.app.tar.gz"), url: `${download}/ThreeStrands_0.56.0_macos_x86_64.app.tar.gz` },
        "linux-x86_64": { signature: await signature("linux-amd64", "app.AppImage"), url: `${download}/ThreeStrands_0.56.0_linux_amd64.AppImage` },
        "windows-x86_64": { signature: await signature("windows-x86_64", "ThreeStrands_setup.exe"), url: `${download}/ThreeStrands_0.56.0_windows_x86_64.exe` },
      },
    });
  });

  it("refuses to publish updates signed by a different key than the app trusts", async (t) => {
    const { directory, output } = await artifacts(t, { signFile: updaterKeyPair().signFile });
    await assert.rejects(prepare(directory, output, "0.56.0"), /signed with a different key/);
    await assert.rejects(readdir(output), { code: "ENOENT" });
  });

  it("refuses an update package changed after it was signed", async (t) => {
    const { directory, output } = await artifacts(t);
    await writeFile(join(directory, "threestrands-release-macos-x86_64", "ThreeStrands.app.tar.gz"), "tampered");
    await assert.rejects(prepare(directory, output, "0.56.0"), /does not verify/);
    await assert.rejects(readdir(output), { code: "ENOENT" });
  });

  it("refuses missing, empty, and malformed update signatures", async (t) => {
    const { directory, output } = await artifacts(t);
    const signature = join(directory, "threestrands-release-linux-amd64", "app.AppImage.sig");
    await writeFile(signature, "");
    await assert.rejects(prepare(directory, output, "0.56.0"), /Malformed update signature/);
    await writeFile(signature, Buffer.from("untrusted comment: x\nAAAA\n").toString("base64"));
    await assert.rejects(prepare(directory, output, "0.56.0"), /Malformed update signature/);
    await rm(signature);
    await assert.rejects(prepare(directory, output, "0.56.0"), /exactly one linux-amd64 \.AppImage\.sig/);
    await assert.rejects(readdir(output), { code: "ENOENT" });
  });

  it("verifies a signature produced by the Tauri CLI", async (t) => {
    // `tauri signer generate` / `tauri signer sign` 2.11.4 output for "hi\n" with a throwaway key.
    const directory = await mkdtemp(join(tmpdir(), "threestrands-signature-test-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const file = join(directory, "a.txt");
    await writeFile(file, "hi\n");
    const key = updaterPublicKey(tauriConfig("0.56.0", "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDE5MTVCQjY0MzcyNUU1NUIKUldSYjVTVTNaTHNWR2JNcWkweFZ2WW1QbmhGMWZQWG04OHVHRzA0MW9Ddko2Z3daMzYvZjdhaFUK"));
    const signature = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSYjVTVTNaTHNWR1N1SkJ4WFYvTi9Pd2J5aGE4ODJWSzdvbzZ2SmtnTkhYZStWSzhaM2xod2JXMkYwOVI3U1BKVWZ4bTA5dXRjRm5qMXNIeDRhTjhGSmhYek5tMWhUaVFVPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkxMjk5ODQyCWZpbGU6YS50eHQKZ1lYa2IvYzRBVUVaemJEaUZ6czVQaEpnK1ZkWmVQYXEwZk0yZ1Rrc2pPY0dKRUFIWDBLN05nT0xPTVljSVVuZllia3pCT3lueGNqaXp2TVlJT0g0Q1E9PQo=";
    await verifyUpdateSignature(file, signature, key);
    await writeFile(file, "hj\n");
    await assert.rejects(verifyUpdateSignature(file, signature, key), /does not verify/);
  });

  it("rejects missing and duplicate installers before creating output", async (t) => {
    const { directory, output } = await artifacts(t);
    const folder = join(directory, "threestrands-release-linux-amd64");
    await rm(join(folder, "app.rpm"));
    await assert.rejects(prepare(directory, output, "0.56.0"), /exactly one linux-amd64 \.rpm/);
    await writeFile(join(folder, "app.rpm"), "rpm");
    await writeFile(join(folder, "duplicate.deb"), "deb");
    await assert.rejects(prepare(directory, output, "0.56.0"), /exactly one linux-amd64 \.deb/);
    await assert.rejects(readdir(output), { code: "ENOENT" });
  });

  it("rejects empty installers, symbolic links, and missing platform artifacts", async (t) => {
    const { directory, output } = await artifacts(t);
    const installer = join(directory, "threestrands-release-macos-aarch64", "arm.dmg");
    await writeFile(installer, "");
    await assert.rejects(prepare(directory, output, "0.56.0"), /Empty installer/);
    await rm(installer);
    await symlink(join(directory, "threestrands-release-macos-x86_64", "intel.dmg"), installer);
    await assert.rejects(prepare(directory, output, "0.56.0"), /exactly one macos-aarch64/);
    await rm(join(directory, "threestrands-release-macos-aarch64"), { recursive: true });
    await assert.rejects(prepare(directory, output, "0.56.0"), { code: "ENOENT" });
  });

  it("leaves a skipped optional platform out of the assets, checksums, and update manifest", async (t) => {
    const { directory, output } = await artifacts(t);
    await rm(join(directory, "threestrands-release-linux-amd64"), { recursive: true });
    await rm(join(directory, "threestrands-release-windows-x86_64"), { recursive: true });
    const names = await prepare(directory, output, "0.56.0", undefined, ["linux-amd64", "windows-x86_64"]);
    assert.deepEqual(names, [
      "ThreeStrands_0.56.0_macos_aarch64.dmg", "ThreeStrands_0.56.0_macos_x86_64.dmg",
      "ThreeStrands_0.56.0_macos_aarch64.app.tar.gz", "ThreeStrands_0.56.0_macos_x86_64.app.tar.gz",
      "latest.json",
    ]);
    const manifest = JSON.parse(await readFile(join(output, "latest.json"), "utf8"));
    assert.deepEqual(Object.keys(manifest.platforms), ["darwin-aarch64", "darwin-x86_64"]);
    assert.doesNotMatch(await readFile(join(output, "SHA256SUMS"), "utf8"), /linux|windows/);
  });

  it("leaves Intel Macs out of the assets, checksums, and update manifest when the release skips them", async (t) => {
    const { directory, output } = await artifacts(t);
    await rm(join(directory, "threestrands-release-macos-x86_64"), { recursive: true });
    const names = await prepare(directory, output, "0.56.0", undefined, ["macos-x86_64"]);
    assert.ok(names.includes("ThreeStrands_0.56.0_macos_aarch64.dmg"));
    assert.equal(names.some((name) => name.includes("x86_64") && name.includes("macos")), false);
    const manifest = JSON.parse(await readFile(join(output, "latest.json"), "utf8"));
    assert.deepEqual(Object.keys(manifest.platforms), ["darwin-aarch64", "linux-x86_64", "windows-x86_64"]);
    assert.doesNotMatch(await readFile(join(output, "SHA256SUMS"), "utf8"), /macos_x86_64/);
  });

  it("still requires a platform that was not skipped, and never skips macOS", async (t) => {
    const { directory, output } = await artifacts(t);
    await rm(join(directory, "threestrands-release-windows-x86_64"), { recursive: true });
    await assert.rejects(prepare(directory, output, "0.56.0", undefined, ["linux-amd64"]), { code: "ENOENT" });
    await assert.rejects(prepare(directory, output, "0.56.0", undefined, ["macos-aarch64", "windows-x86_64"]), /cannot skip macos-aarch64/);
    await assert.rejects(readdir(output), { code: "ENOENT" });
  });

  it("rejects invalid versions and existing output directories", async (t) => {
    const { directory, output } = await artifacts(t);
    await assert.rejects(prepare(directory, output, "../escape"), /Invalid release asset version/);
    await mkdir(output);
    await writeFile(join(output, "stale.dmg"), "stale");
    await assert.rejects(prepare(directory, output, "0.56.0"), { code: "EEXIST" });
  });
});

describe("host release bump", () => {
  const fix = { "clearer-fix.md": "### Fixed\n- A clearer fix.\n" };
  const bump = (fixture, ...args) => fixture.run([], ["scripts/release.mjs", "bump", ...args]);

  it("writes the next version and stamped changelog with the host's date, deletes change files, prints only the tag, and leaves Git alone", async (t) => {
    const fixture = await releaseCommand(t, { changes: fix });
    const result = bump(fixture, "2026-11-03");
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "v0.55.1\n");
    assert.equal(await fixture.read("package.json"), sources("0.55.1").packageJson);
    assert.equal(await fixture.read("src-tauri/Cargo.lock"), sources("0.55.1").cargoLock);
    assert.match(await fixture.read("CHANGELOG.md"), /^## Unreleased\n\n## 0\.55\.1 — 2026-11-03\n\n### Fixed\n- A clearer fix\.\n\n## 0\.55\.0/m);
    assert.equal(await fixture.exists("changes/clearer-fix.md"), false);
    assert.deepEqual(await fixture.commands(), []);
  });

  for (const [args, tag] of [[["--major"], "v1.0.0"], [["0.55.0"], "v0.55.0"]]) {
    it(`passes ${args.join(" ")} through to the version choice`, async (t) => {
      const fixture = await releaseCommand(t, { changes: fix });
      const result = bump(fixture, "2026-11-03", ...args);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `${tag}\n`);
    });
  }

  it("sends the docs warning to stderr so stdout stays the tag", async (t) => {
    const fixture = await releaseCommand(t, { changes: { "names.md": "### New\n- **Missing Name**.\n" } });
    const result = bump(fixture, "2026-11-03");
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /Not mentioned in site\/docs or site\/index\.html: Missing Name\n/);
    assert.equal(result.stdout, "v0.56.0\n");
  });

  it("rejects a malformed date and inconsistent versions without output or changes", async (t) => {
    const badDate = await releaseCommand(t, { changes: fix });
    const dateResult = bump(badDate, "11/03/2026");
    assert.equal(dateResult.status, 1);
    assert.match(dateResult.stderr, /Invalid release date/);
    assert.equal(dateResult.stdout, "");
    assert.equal(await badDate.exists("changes/clearer-fix.md"), true);

    const fixture = await releaseCommand(t, { changes: fix });
    await writeFile(join(fixture.directory, "src-tauri/Cargo.lock"), sources("0.55.3").cargoLock);
    const mismatch = bump(fixture, "2026-11-03");
    assert.equal(mismatch.status, 1);
    assert.match(mismatch.stderr, /Cargo\.lock \(threestrands\) has version 0\.55\.3/);
    assert.equal(mismatch.stdout, "");
    assert.equal(await fixture.read("package.json"), sources("0.55.0").packageJson);
  });
});
