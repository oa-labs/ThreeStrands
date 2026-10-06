import { spawnSync } from "node:child_process";
import { createHash, createPublicKey, verify } from "node:crypto";
import { createReadStream } from "node:fs";
import { appendFile, copyFile, mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";
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

  updaterPublicKey(sources.tauriConfig);
  const prerelease = version.includes("-beta.");
  return { version, channel: prerelease ? "beta" : "stable", prerelease };
}

/**
 * Installed copies trust only this key, so a release without it could never
 * be installed as an update. Returns the decoded raw Ed25519 key and its id.
 */
export function updaterPublicKey(tauriConfig) {
  const encoded = JSON.parse(tauriConfig).plugins?.updater?.pubkey;
  if (typeof encoded !== "string" || !encoded.trim()) {
    throw new Error("src-tauri/tauri.conf.json needs plugins.updater.pubkey before a release can ship updates");
  }
  const key = Buffer.from(Buffer.from(encoded, "base64").toString("utf8").split("\n")[1] ?? "", "base64");
  if (key.length !== 42 || key.subarray(0, 2).toString() !== "Ed") {
    throw new Error("plugins.updater.pubkey is not a minisign public key from `tauri signer generate`");
  }
  return { keyId: key.subarray(2, 10), publicKey: createPublicKey({
    key: Buffer.concat([ED25519_SPKI_PREFIX, key.subarray(10)]), format: "der", type: "spki",
  }) };
}

const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

async function fileDigest(path, algorithm) {
  const hash = createHash(algorithm);
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest();
}

/**
 * Verifies a `tauri signer` (minisign) signature over `path` exactly as the
 * installed updater will, so a mismatched signing secret fails the release
 * instead of every user's update.
 */
export async function verifyUpdateSignature(path, signature, { keyId, publicKey }) {
  const lines = Buffer.from(signature, "base64").toString("utf8").split("\n");
  const raw = Buffer.from(lines[1] ?? "", "base64");
  const trustedComment = lines[2]?.startsWith("trusted comment: ") ? lines[2].slice("trusted comment: ".length) : null;
  const algorithm = raw.subarray(0, 2).toString();
  if (raw.length !== 74 || !["Ed", "ED"].includes(algorithm) || trustedComment === null) {
    throw new Error(`Malformed update signature for ${basename(path)}`);
  }
  if (!raw.subarray(2, 10).equals(keyId)) {
    throw new Error(`${basename(path)} was signed with a different key than plugins.updater.pubkey`);
  }
  const message = algorithm === "ED" ? await fileDigest(path, "blake2b512") : await readFile(path);
  const signed = raw.subarray(10);
  if (!verify(null, message, publicKey, signed)
    || !verify(null, Buffer.concat([signed, Buffer.from(trustedComment)]), publicKey, Buffer.from(lines[3] ?? "", "base64"))) {
    throw new Error(`Update signature does not verify for ${basename(path)}`);
  }
}

// Only these files become release assets. Build diagnostics and the
// Linux-only checksum file stay in Actions artifacts; hashes here cover all OSes.
const installers = [
  ["macos-aarch64", ".dmg"],
  ["macos-x86_64", ".dmg"],
  ["linux-amd64", ".deb"],
  ["linux-amd64", ".rpm"],
  ["linux-amd64", ".AppImage"],
  // In-place update packages. The AppImage is its own update package.
  ["macos-aarch64", ".app.tar.gz"],
  ["macos-x86_64", ".app.tar.gz"],
];

// The updater's platform keys, each paired with the asset it downloads.
// deb and rpm installs also resolve `linux-x86_64`, but only to learn that an
// update exists; the app sends those users to the release page instead.
const updatePlatforms = [
  ["darwin-aarch64", "macos-aarch64", ".app.tar.gz"],
  ["darwin-x86_64", "macos-x86_64", ".app.tar.gz"],
  ["linux-x86_64", "linux-amd64", ".AppImage"],
];

export const UPDATE_MANIFEST = "latest.json";
const RELEASE_DOWNLOADS = "https://github.com/oa-labs/ThreeStrands/releases/download";

async function onlyFile(directory, platform, extension) {
  const matches = (await readdir(directory, { withFileTypes: true }))
    .filter((entry) => entry.isFile() && entry.name.endsWith(extension));
  if (matches.length !== 1) {
    throw new Error(`Expected exactly one ${platform} ${extension} file; found ${matches.length}`);
  }
  return resolve(directory, matches[0].name);
}

export async function prepareReleaseAssets(inputDir, outputDir, version, { updaterKey, now = new Date() }) {
  if (!releaseVersion.test(version)) throw new Error("Invalid release asset version");
  const selected = [];
  for (const [platform, extension] of installers) {
    const source = await onlyFile(resolve(inputDir, `threestrands-release-${platform}`), platform, extension);
    if ((await stat(source)).size === 0) throw new Error(`Empty installer: ${basename(source)}`);
    selected.push({ platform, extension, source, name: `ThreeStrands_${version}_${platform.replaceAll("-", "_")}${extension}` });
  }

  const platforms = {};
  for (const [target, platform, extension] of updatePlatforms) {
    const asset = selected.find((item) => item.platform === platform && item.extension === extension);
    const signaturePath = await onlyFile(resolve(inputDir, `threestrands-release-${platform}`), platform, `${extension}.sig`);
    const signature = (await readFile(signaturePath, "utf8")).trim();
    await verifyUpdateSignature(asset.source, signature, updaterKey);
    platforms[target] = { signature, url: `${RELEASE_DOWNLOADS}/v${version}/${asset.name}` };
  }
  const manifest = `${JSON.stringify({ version, pub_date: now.toISOString(), platforms }, null, 2)}\n`;

  // Require a new directory so a rerun cannot accidentally publish stale files.
  await mkdir(outputDir, { recursive: false });
  const checksums = [];
  for (const { source, name } of selected) {
    const destination = resolve(outputDir, name);
    await copyFile(source, destination);
    checksums.push(`${(await fileDigest(destination, "sha256")).toString("hex")}  ${name}\n`);
  }
  await writeFile(resolve(outputDir, UPDATE_MANIFEST), manifest);
  await writeFile(resolve(outputDir, "SHA256SUMS"), checksums.join(""));
  return [...selected.map(({ name }) => name), UPDATE_MANIFEST];
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
    const updaterKey = updaterPublicKey((await readReleaseSources(root)).tauriConfig);
    console.log((await prepareReleaseAssets(argument, outputDir, version, { updaterKey })).join("\n"));
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
