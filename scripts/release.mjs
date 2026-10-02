import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { appendFile, copyFile, mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const number = "(?:0|[1-9][0-9]*)";
const releaseVersion = new RegExp(`^${number}\\.${number}\\.${number}(?:-beta\\.${number})?$`);

function tomlField(section, field) {
  return section?.match(new RegExp(`^${field}\\s*=\\s*"([^"\\r\\n]+)"`, "m"))?.[1];
}

export function releaseMetadata(tag, sources) {
  const version = tag?.startsWith("v") ? tag.slice(1) : "";
  if (!releaseVersion.test(version)) {
    throw new Error("Release tags must be vMAJOR.MINOR.PATCH or vMAJOR.MINOR.PATCH-beta.N");
  }

  const cargoPackage = sources.cargoToml.match(/^\[package\]\s*\n([^]*?)(?=^\[|(?![^]))/m)?.[1];
  const lockPackages = sources.cargoLock.split(/^\[\[package\]\]\s*$/m)
    .filter((section) => tomlField(section, "name") === "threestrands");
  if (tomlField(cargoPackage, "name") !== "threestrands" || lockPackages.length !== 1) {
    throw new Error("Expected exactly one threestrands application package in Cargo.toml and Cargo.lock");
  }

  const versions = {
    "package.json": JSON.parse(sources.packageJson).version,
    "src-tauri/tauri.conf.json": JSON.parse(sources.tauriConfig).version,
    "src-tauri/Cargo.toml": tomlField(cargoPackage, "version"),
    "src-tauri/Cargo.lock (threestrands)": tomlField(lockPackages[0], "version"),
  };
  for (const [file, actual] of Object.entries(versions)) {
    if (actual !== version) {
      throw new Error(`${file} has version ${actual ?? "missing"}; tag ${tag} requires ${version}`);
    }
  }

  const prerelease = version.includes("-beta.");
  return { version, channel: prerelease ? "beta" : "stable", prerelease };
}

// Only these five installers become release assets. Build diagnostics and the
// Linux-only checksum file stay in Actions artifacts; hashes here cover all OSes.
const installers = [
  ["macos-aarch64", ".dmg"],
  ["macos-x86_64", ".dmg"],
  ["linux-amd64", ".deb"],
  ["linux-amd64", ".rpm"],
  ["linux-amd64", ".AppImage"],
];

export async function prepareReleaseAssets(inputDir, outputDir, version) {
  if (!releaseVersion.test(version)) throw new Error("Invalid release asset version");
  const selected = [];
  for (const [platform, extension] of installers) {
    const directory = resolve(inputDir, `threestrands-release-${platform}`);
    const matches = (await readdir(directory, { withFileTypes: true }))
      .filter((entry) => entry.isFile() && entry.name.endsWith(extension));
    if (matches.length !== 1) {
      throw new Error(`Expected exactly one ${platform} ${extension} installer; found ${matches.length}`);
    }
    const source = resolve(directory, matches[0].name);
    if ((await stat(source)).size === 0) throw new Error(`Empty installer: ${matches[0].name}`);
    selected.push({ source, name: `ThreeStrands_${version}_${platform.replaceAll("-", "_")}${extension}` });
  }

  // Require a new directory so a rerun cannot accidentally publish stale files.
  await mkdir(outputDir, { recursive: false });
  const checksums = [];
  for (const { source, name } of selected) {
    const destination = resolve(outputDir, name);
    await copyFile(source, destination);
    const hash = createHash("sha256");
    for await (const chunk of createReadStream(destination)) hash.update(chunk);
    checksums.push(`${hash.digest("hex")}  ${name}\n`);
  }
  await writeFile(resolve(outputDir, "SHA256SUMS"), checksums.join(""));
  return selected.map(({ name }) => name);
}

async function readReleaseSources(directory) {
  const paths = ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock"];
  const [packageJson, tauriConfig, cargoToml, cargoLock] = await Promise.all(
    paths.map((path) => readFile(resolve(directory, path), "utf8")),
  );
  return { packageJson, tauriConfig, cargoToml, cargoLock };
}

export async function tagRelease(directory = root) {
  const sources = await readReleaseSources(directory);
  const tag = `v${JSON.parse(sources.packageJson).version}`;
  releaseMetadata(tag, sources);

  const git = (args, capture = false) => {
    const result = spawnSync("git", args, {
      cwd: directory,
      encoding: "utf8",
      stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit",
    });
    if (result.error) throw result.error;
    if (result.status !== 0) {
      throw new Error(`git ${args.join(" ")} failed (${result.signal ?? `exit ${result.status}`})`);
    }
    return result.stdout?.trim();
  };

  if (git(["branch", "--show-current"], true) !== "master") {
    throw new Error("Switch to master before releasing; the release tag must point to the branch being pushed");
  }
  if (git(["status", "--porcelain"], true)) {
    throw new Error("Commit or stash your changes before releasing; the release version must be committed");
  }

  console.log(`Releasing ThreeStrands ${tag}`);
  git(["push", "origin", "master"]);
  git(["tag", "-s", tag, "-m", `ThreeStrands ${tag}`]);
  git(["push", "origin", tag]);
  return tag;
}

async function main() {
  const [command, argument, outputDir, version] = process.argv.slice(2);
  if (command === "validate") {
    const metadata = releaseMetadata(argument, await readReleaseSources(root));
    if (process.env.GITHUB_OUTPUT) {
      await appendFile(process.env.GITHUB_OUTPUT, Object.entries(metadata)
        .map(([name, value]) => `${name}=${value}\n`).join(""));
    }
    console.log(JSON.stringify(metadata));
  } else if (command === "prepare-assets" && argument && outputDir && version) {
    console.log((await prepareReleaseAssets(argument, outputDir, version)).join("\n"));
  } else if (command === "tag") {
    await tagRelease();
  } else {
    throw new Error("Usage: node scripts/release.mjs validate TAG | prepare-assets INPUT_DIR OUTPUT_DIR VERSION | tag");
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
