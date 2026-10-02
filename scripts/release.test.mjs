import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, mkdtemp, mkdir, readFile, readdir, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { describe, it } from "node:test";
import { prepareReleaseAssets, releaseMetadata } from "./release.mjs";

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
});

function sources(version = "0.56.0") {
  return {
    packageJson: JSON.stringify({ version }),
    tauriConfig: JSON.stringify({ version }),
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

async function releaseCommand(t, { version = "0.56.0", branch = "master", dirty = "", failAt = "" } = {}) {
  const directory = await mkdtemp(join(tmpdir(), "threestrands release test "));
  t.after(() => rm(directory, { recursive: true, force: true }));
  for (const folder of ["scripts", "src-tauri", "bin"]) await mkdir(join(directory, folder));
  const fixture = sources(version);
  for (const [path, content] of [
    ["package.json", fixture.packageJson], ["src-tauri/tauri.conf.json", fixture.tauriConfig],
    ["src-tauri/Cargo.toml", fixture.cargoToml], ["src-tauri/Cargo.lock", fixture.cargoLock],
  ]) await writeFile(join(directory, path), content);
  await writeFile(join(directory, "scripts/release.mjs"), await readFile(new URL("./release.mjs", import.meta.url)));

  const log = join(directory, "git-commands.jsonl");
  const fakeGit = join(directory, "bin/git");
  await writeFile(fakeGit, `#!/usr/bin/env node
import { appendFileSync } from "node:fs";
const args = process.argv.slice(2);
appendFileSync(process.env.RELEASE_TEST_LOG, JSON.stringify(args) + "\\n");
if (args[0] === "branch") console.log(process.env.RELEASE_TEST_BRANCH);
if (args[0] === "status") console.log(process.env.RELEASE_TEST_DIRTY);
const action = args[0] === "tag" ? "sign-tag" : args[0] === "push" ? (args[2] === "master" ? "push-master" : "push-tag") : "read";
if (action === process.env.RELEASE_TEST_FAIL_AT) process.exit(7);
`);
  await chmod(fakeGit, 0o755);
  const packageJson = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
  const [runtime, script, ...args] = packageJson.scripts.release.split(" ");
  assert.equal(runtime, "node");
  return {
    directory,
    run: () => spawnSync(process.execPath, [script, ...args], {
      cwd: directory,
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: `${join(directory, "bin")}${delimiter}${process.env.PATH}`,
        RELEASE_TEST_LOG: log,
        RELEASE_TEST_BRANCH: branch,
        RELEASE_TEST_DIRTY: dirty,
        RELEASE_TEST_FAIL_AT: failAt,
      },
    }),
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

describe("pnpm release command", () => {
  const commands = (version) => [
    ["branch", "--show-current"], ["status", "--porcelain"],
    ["push", "origin", "master"],
    ["tag", "-s", `v${version}`, "-m", `ThreeStrands v${version}`],
    ["push", "origin", `v${version}`],
  ];

  for (const version of ["0.56.0", "0.57.0-beta.1"]) {
    it(`pushes master, signs the current ${version} tag, and pushes only that tag`, async (t) => {
      const fixture = await releaseCommand(t, { version });
      const result = fixture.run();
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(await fixture.commands(), commands(version));
    });
  }

  for (const [failAt, index] of [["push-master", 2], ["sign-tag", 3], ["push-tag", 4]]) {
    it(`stops when ${failAt} fails`, async (t) => {
      const fixture = await releaseCommand(t, { failAt });
      const result = fixture.run();
      assert.equal(result.status, 1);
      assert.match(result.stderr, /failed \(exit 7\)/);
      assert.deepEqual(await fixture.commands(), commands("0.56.0").slice(0, index + 1));
    });
  }

  it("requires the master branch and committed changes before pushing", async (t) => {
    const branch = await releaseCommand(t, { branch: "feature" });
    assert.match(branch.run().stderr, /Switch to master/);
    assert.deepEqual(await branch.commands(), commands("0.56.0").slice(0, 1));
    const dirty = await releaseCommand(t, { dirty: " M package.json" });
    assert.match(dirty.run().stderr, /Commit or stash/);
    assert.deepEqual(await dirty.commands(), commands("0.56.0").slice(0, 2));
  });

  it("rejects inconsistent application versions before invoking Git", async (t) => {
    const fixture = await releaseCommand(t);
    await writeFile(join(fixture.directory, "src-tauri/Cargo.lock"), sources("0.55.3").cargoLock);
    const result = fixture.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Cargo\.lock.*requires 0\.56\.0/);
    assert.deepEqual(await fixture.commands(), []);
  });
});

async function artifacts(t) {
  const directory = await mkdtemp(join(tmpdir(), "threestrands-release-test-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const files = [
    ["macos-aarch64", "arm.dmg"], ["macos-x86_64", "intel.dmg"],
    ["linux-amd64", "app.deb"], ["linux-amd64", "app.rpm"], ["linux-amd64", "app.AppImage"],
  ];
  for (const [platform, name] of files) {
    const folder = join(directory, `threestrands-release-${platform}`);
    await mkdir(folder, { recursive: true });
    await writeFile(join(folder, name), `fixture ${platform} ${name}`);
  }
  return { directory, output: join(directory, "output") };
}

describe("release installer collection", () => {
  it("collects all five installers with versioned names and verifiable hashes, excluding diagnostics", async (t) => {
    const { directory, output } = await artifacts(t);
    await writeFile(join(directory, "threestrands-release-linux-amd64", "SHA256SUMS"), "old hashes");
    await writeFile(join(directory, "threestrands-release-linux-amd64", "native-library-dependencies.txt"), "diagnostics");
    const names = await prepareReleaseAssets(directory, output, "0.56.0-beta.1");
    assert.deepEqual(names, [
      "ThreeStrands_0.56.0-beta.1_macos_aarch64.dmg", "ThreeStrands_0.56.0-beta.1_macos_x86_64.dmg",
      "ThreeStrands_0.56.0-beta.1_linux_amd64.deb", "ThreeStrands_0.56.0-beta.1_linux_amd64.rpm",
      "ThreeStrands_0.56.0-beta.1_linux_amd64.AppImage",
    ]);
    assert.deepEqual((await readdir(output)).sort(), [...names, "SHA256SUMS"].sort());
    const expected = [];
    for (const name of names) {
      const digest = createHash("sha256").update(await readFile(join(output, name))).digest("hex");
      expected.push(`${digest}  ${name}\n`);
    }
    assert.equal(await readFile(join(output, "SHA256SUMS"), "utf8"), expected.join(""));
  });

  it("rejects missing and duplicate installers before creating output", async (t) => {
    const { directory, output } = await artifacts(t);
    const folder = join(directory, "threestrands-release-linux-amd64");
    await rm(join(folder, "app.rpm"));
    await assert.rejects(prepareReleaseAssets(directory, output, "0.56.0"), /exactly one linux-amd64 \.rpm/);
    await writeFile(join(folder, "app.rpm"), "rpm");
    await writeFile(join(folder, "duplicate.deb"), "deb");
    await assert.rejects(prepareReleaseAssets(directory, output, "0.56.0"), /exactly one linux-amd64 \.deb/);
    await assert.rejects(readdir(output), { code: "ENOENT" });
  });

  it("rejects empty installers, symbolic links, and missing platform artifacts", async (t) => {
    const { directory, output } = await artifacts(t);
    const installer = join(directory, "threestrands-release-macos-aarch64", "arm.dmg");
    await writeFile(installer, "");
    await assert.rejects(prepareReleaseAssets(directory, output, "0.56.0"), /Empty installer/);
    await rm(installer);
    await symlink(join(directory, "threestrands-release-macos-x86_64", "intel.dmg"), installer);
    await assert.rejects(prepareReleaseAssets(directory, output, "0.56.0"), /exactly one macos-aarch64/);
    await rm(join(directory, "threestrands-release-macos-aarch64"), { recursive: true });
    await assert.rejects(prepareReleaseAssets(directory, output, "0.56.0"), { code: "ENOENT" });
  });

  it("rejects invalid versions and existing output directories", async (t) => {
    const { directory, output } = await artifacts(t);
    await assert.rejects(prepareReleaseAssets(directory, output, "../escape"), /Invalid release asset version/);
    await mkdir(output);
    await writeFile(join(output, "stale.dmg"), "stale");
    await assert.rejects(prepareReleaseAssets(directory, output, "0.56.0"), { code: "EEXIST" });
  });
});
