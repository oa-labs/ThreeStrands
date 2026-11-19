import { spawnSync } from "node:child_process";
import { createHash, createPublicKey, verify } from "node:crypto";
import { createReadStream } from "node:fs";
import { appendFile, copyFile, mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { collectChanges, compareVersions, nextVersion, releaseNotes, stampRelease, undocumentedTerms } from "./changelog.mjs";

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
 * `sources` with the application version set to `version` in all four files,
 * leaving every other field, dependency, and byte of formatting alone.
 */
export function setAppVersion(sources, version) {
  if (!releaseVersion.test(version)) throw new Error(`Invalid application version ${version}`);
  const once = (text, pattern, file) => {
    let count = 0;
    const result = text.replace(pattern, (_, before, after) => {
      count += 1;
      return `${before}${version}${after}`;
    });
    if (count !== 1) throw new Error(`Expected one application version in ${file}; found ${count}`);
    return result;
  };
  // The top-level "version" is the first key at two-space indentation.
  const jsonVersion = /^( {2}"version": ")[^"\r\n]*(",?)$/m;
  // Cargo.toml's [package] table: its lines up to the next table heading.
  const cargoToml = sources.cargoToml.replace(/^\[package\]\s*\n[^]*?(?=^\[|(?![^]))/m, (section) =>
    once(section, /^(version\s*=\s*")[^"\r\n]+(")/m, "src-tauri/Cargo.toml [package]"));
  const cargoLock = once(sources.cargoLock,
    /^(\[\[package\]\]\s*\nname = "threestrands"\s*\nversion = ")[^"\r\n]+(")/gm, "src-tauri/Cargo.lock");
  const updated = {
    ...sources,
    packageJson: once(sources.packageJson, jsonVersion, "package.json"),
    tauriConfig: once(sources.tauriConfig, jsonVersion, "src-tauri/tauri.conf.json"),
    cargoToml,
    cargoLock,
  };
  releaseMetadata(`v${version}`, updated);
  return updated;
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
  ["windows-x86_64", ".exe"],
  // In-place update packages. The AppImage is its own update package, and on
  // Windows the NSIS .exe is both the installer and the update package.
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
  ["windows-x86_64", "windows-x86_64", ".exe"],
];

// Apple Silicon macOS is the primary target and always ships. A release may go
// out without the others: Intel Macs when its tag opts out of them, and Linux
// or Windows when their build fails. The workflow names each one it
// leaves out.
export const OPTIONAL_PLATFORMS = ["macos-x86_64", "linux-amd64", "windows-x86_64"];

// Every release builds for Intel Macs unless its tag carries this line, which
// `pnpm release --no-intel` and `scripts/release.sh --no-intel` add, for when
// the Intel build can't run.
export const INTEL_SKIP_LINE = "Intel: skipped";

/** The `-m` arguments for the signed release tag. */
export function releaseTagMessage(tag, { intel = true } = {}) {
  return ["-m", `ThreeStrands ${tag}`, ...(intel ? [] : ["-m", INTEL_SKIP_LINE])];
}

/** Whether a release tag's message leaves Intel Mac builds in. */
export function includesIntel(tagMessage) {
  return !(tagMessage ?? "").split("\n").some((line) => line.trim() === INTEL_SKIP_LINE);
}

const MACOS_BUILDS = [
  // Tests run once, natively, on the Apple Silicon runner; the Intel build
  // would only repeat them, since its tests also run as Apple Silicon.
  { target: "aarch64-apple-darwin", arch: "aarch64", binary_arch: "arm64", tests: true },
  { target: "x86_64-apple-darwin", arch: "x86_64", binary_arch: "x86_64", tests: false },
];

/** The macOS build matrix for a release, Apple Silicon first. */
export function macosBuilds({ intel }) {
  return MACOS_BUILDS.filter((build) => intel || build.arch === "aarch64");
}

/**
 * The message of an annotated release tag, without its signature. A
 * lightweight tag has none, so it builds every platform.
 */
export function readTagMessage(directory, tag) {
  const result = spawnSync("git", ["for-each-ref", "--format=%(objecttype)%00%(contents:subject)%00%(contents:body)", `refs/tags/${tag}`], {
    cwd: directory, encoding: "utf8",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`Unable to read tag ${tag} (exit ${result.status})`);
  if (!result.stdout.trim()) throw new Error(`Tag ${tag} is not in this checkout; fetch it first`);
  const [type, subject, body] = result.stdout.replace(/\n$/, "").split("\0");
  return type === "tag" ? `${subject}\n\n${body}`.trim() : "";
}

/**
 * The note that tops the release notes of a release without Intel Mac
 * installers, pointing at the latest published release that has them.
 */
export function intelNotice({ lastIntelTag, repository } = {}) {
  const latest = lastIntelTag && repository
    ? ` Intel Macs can install [${lastIntelTag}](https://github.com/${repository}/releases/tag/${lastIntelTag}), the latest release that includes them.`
    : "";
  return `> [!NOTE]\n> This release has no installer for Intel Macs.${latest} Intel Macs are offered updates only by releases that include them.\n`;
}

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

export async function prepareReleaseAssets(inputDir, outputDir, version, { updaterKey, now = new Date(), skip = [] }) {
  if (!releaseVersion.test(version)) throw new Error("Invalid release asset version");
  for (const platform of skip) {
    if (!OPTIONAL_PLATFORMS.includes(platform)) throw new Error(`A release cannot skip ${platform}`);
  }
  const selected = [];
  for (const [platform, extension] of installers.filter(([platform]) => !skip.includes(platform))) {
    const source = await onlyFile(resolve(inputDir, `threestrands-release-${platform}`), platform, extension);
    if ((await stat(source)).size === 0) throw new Error(`Empty installer: ${basename(source)}`);
    selected.push({ platform, extension, source, name: `ThreeStrands_${version}_${platform.replaceAll("-", "_")}${extension}` });
  }

  const platforms = {};
  for (const [target, platform, extension] of updatePlatforms.filter(([, platform]) => !skip.includes(platform))) {
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

const VERSION_FILES = {
  packageJson: "package.json",
  tauriConfig: "src-tauri/tauri.conf.json",
  cargoToml: "src-tauri/Cargo.toml",
  cargoLock: "src-tauri/Cargo.lock",
};

async function readReleaseSources(directory) {
  const entries = await Promise.all(Object.entries(VERSION_FILES)
    .map(async ([key, path]) => [key, await readFile(resolve(directory, path), "utf8")]));
  return Object.fromEntries(entries);
}

/** Where each change adds its changelog entries; see parseChangeFile in changelog.mjs. */
export const CHANGES_DIR = "changes";

/** The change files in `changes/`, sorted by name; README.md explains the folder. */
async function readChanges(directory) {
  let names;
  try {
    names = await readdir(resolve(directory, CHANGES_DIR));
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw error;
  }
  const files = names.filter((name) => name.endsWith(".md") && name !== "README.md").sort();
  return Promise.all(files.map(async (name) => ({
    file: `${CHANGES_DIR}/${name}`,
    source: await readFile(resolve(directory, CHANGES_DIR, name), "utf8"),
  })));
}

/** CHANGELOG.md with every change file folded into `## Unreleased`. */
async function pendingChangelog(directory) {
  const source = await readFile(resolve(directory, "CHANGELOG.md"), "utf8");
  const changes = await readChanges(directory);
  return { source, changes, collected: collectChanges(source, changes) };
}

/**
 * Works out a release without changing anything: the version (next after
 * package.json from the pending entries, the next major with `major`, or an
 * explicit `version`, where the current version re-cuts it), the files to
 * write (version files, CHANGELOG.md, and help pages marked `reviewed: next`),
 * and the change files to delete. Throws before any change when the
 * four version files disagree or an entry is malformed.
 */
export async function planRelease(directory, { date, version, major = false } = {}) {
  if (!/^[0-9]{4}-[0-9]{2}-[0-9]{2}$/.test(date ?? "")) throw new Error(`Invalid release date ${date}; expected YYYY-MM-DD`);
  if (version && major) throw new Error("Pass a version or --major, not both");
  const sources = await readReleaseSources(directory);
  const current = JSON.parse(sources.packageJson).version;
  releaseMetadata(`v${current}`, sources);

  const { source, changes, collected } = await pendingChangelog(directory);
  const target = version ?? nextVersion(current, collected, { major });
  if (compareVersions(target, current) < 0) throw new Error(`${target} is older than the current version ${current}`);

  const writes = {};
  if (target !== current) {
    const updated = setAppVersion(sources, target);
    for (const [key, path] of Object.entries(VERSION_FILES)) writes[path] = updated[key];
  }
  const stamped = stampRelease(collected, target, date);
  if (stamped !== source) writes["CHANGELOG.md"] = stamped;
  Object.assign(writes, await stampedDocs(directory, target));
  return { tag: `v${target}`, writes, deletes: changes.map((change) => change.file) };
}

const DOCS_DIR = "site/docs";

/**
 * A help page with its `reviewed: next` front matter set to `version`; the
 * page is unchanged when it was not reviewed for this release. Betas are not
 * on the website, so they leave `next` for the stable release.
 */
export function stampReviewed(source, version) {
  if (version.includes("-beta.")) return source;
  const frontMatter = /^---\n[^]*?\n---\n/.exec(source)?.[0];
  if (!frontMatter) return source;
  return frontMatter.replace(/^reviewed:[ \t]*next[ \t]*$/m, `reviewed: ${version}`) + source.slice(frontMatter.length);
}

async function stampedDocs(directory, version) {
  let names;
  try {
    names = (await readdir(resolve(directory, DOCS_DIR))).filter((name) => name.endsWith(".md")).sort();
  } catch (error) {
    if (error.code === "ENOENT") return {};
    throw error;
  }
  const writes = {};
  for (const name of names) {
    const path = `${DOCS_DIR}/${name}`;
    const source = await readFile(resolve(directory, path), "utf8");
    const stamped = stampReviewed(source, version);
    if (stamped !== source) writes[path] = stamped;
  }
  return writes;
}

async function writeRelease(directory, { writes, deletes }) {
  for (const [path, content] of Object.entries(writes)) await writeFile(resolve(directory, path), content);
  for (const path of deletes) await rm(resolve(directory, path));
  return [...Object.keys(writes), ...deletes];
}

// Bold changelog names the site words differently, so the docs check skips them.
export const DOCS_CHECK_IGNORED = [];

/** Unreleased UI names missing from the help pages and home page; see undocumentedTerms. */
export async function docsCoverage(directory = root) {
  const docs = resolve(directory, "site/docs");
  const pages = (await readdir(docs)).filter((name) => name.endsWith(".md")).map((name) => resolve(docs, name));
  const site = await Promise.all([...pages, resolve(directory, "site/index.html")].map((path) => readFile(path, "utf8")));
  return undocumentedTerms((await pendingChangelog(directory)).collected, site.join("\n"), DOCS_CHECK_IGNORED);
}

function warnUndocumented(terms) {
  if (!terms.length) return;
  console.warn(`Not mentioned in site/docs or site/index.html: ${terms.join(", ")}`);
  console.warn("Update the docs if these are new, or add them to DOCS_CHECK_IGNORED in scripts/release.mjs.");
}

/** Today's date where the release is cut, as YYYY-MM-DD. */
function localDate(now = new Date()) {
  return [now.getFullYear(), now.getMonth() + 1, now.getDate()].map((part) => String(part).padStart(2, "0")).join("-");
}

/**
 * The release from tagRelease without Git: scripts/release.sh runs this in
 * the development VM, then commits, signs the tag, and pushes on the host.
 * Writes the version files and stamped CHANGELOG.md, deletes the change
 * files, and returns the tag. `date` is the host's YYYY-MM-DD.
 */
export async function bumpRelease(directory, options) {
  const plan = await planRelease(directory, options);
  warnUndocumented(await docsCoverage(directory));
  await writeRelease(directory, plan);
  return plan.tag;
}

export async function tagRelease(directory = root, options = {}) {
  const plan = await planRelease(directory, { date: localDate(), ...options });
  const { tag } = plan;

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
    throw new Error("Commit or stash your changes before releasing; the release starts from what master has");
  }
  if (git(["tag", "--list", tag], true)) {
    throw new Error(`Tag ${tag} already exists; to re-cut it, delete it first (see DEVELOPERS.md)`);
  }
  warnUndocumented(await docsCoverage(directory));

  const intel = options.intel !== false;
  console.log(`Releasing ThreeStrands ${tag}${intel ? "" : " without Intel Macs"}`);
  const paths = await writeRelease(directory, plan);
  if (paths.length) {
    git(["add", "-A", "--", ...paths]);
    git(["commit", "-m", `Release ${tag}`]);
  }
  git(["push", "primary", "master"]);
  git(["tag", "-s", tag, ...releaseTagMessage(tag, { intel })]);
  git(["push", "primary", tag]);
  return tag;
}

/** `[--major | VERSION]`, the version choice shared by `tag` and `bump`. */
function versionOptions(args) {
  if (args.length > 1) throw new Error("Pass at most one of --major or VERSION");
  if (args[0] === "--major") return { major: true };
  if (args[0] === undefined) return {};
  if (!releaseVersion.test(args[0])) throw new Error(`Invalid version ${args[0]}; expected MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-beta.N`);
  return { version: args[0] };
}

async function main() {
  const [command, argument, outputDir, version] = process.argv.slice(2);
  const rest = process.argv.slice(3);
  if (command === "validate") {
    const intel = includesIntel(readTagMessage(root, argument));
    const metadata = {
      ...releaseMetadata(argument, await readReleaseSources(root)),
      intel,
      macos_builds: JSON.stringify(macosBuilds({ intel })),
    };
    await pendingChangelog(root);
    if (process.env.GITHUB_OUTPUT) {
      await appendFile(process.env.GITHUB_OUTPUT, Object.entries(metadata)
        .map(([name, value]) => `${name}=${value}\n`).join(""));
    }
    console.log(JSON.stringify(metadata));
  } else if (command === "prepare-assets" && argument && outputDir && version) {
    const updaterKey = updaterPublicKey((await readReleaseSources(root)).tauriConfig);
    // Space- or comma-separated platforms whose build failed, set by the workflow.
    const skip = (process.env.RELEASE_SKIPPED_PLATFORMS ?? "").split(/[\s,]+/).filter(Boolean);
    console.log((await prepareReleaseAssets(argument, outputDir, version, { updaterKey, skip })).join("\n"));
  } else if (command === "notes" && argument && outputDir) {
    const publishedTagsFile = outputDir; // Published (non-draft) release tags, one per line.
    const published = (await readFile(publishedTagsFile, "utf8")).split("\n").filter(Boolean);
    const repository = process.env.GITHUB_REPOSITORY;
    // RELEASE_INTEL comes from the validate job; anything but "true" means no Intel build.
    if (process.env.RELEASE_INTEL !== "true") {
      process.stdout.write(`${intelNotice({ lastIntelTag: process.env.RELEASE_LAST_INTEL_TAG || undefined, repository })}\n`);
    }
    process.stdout.write(releaseNotes(await readFile(resolve(root, "CHANGELOG.md"), "utf8"), argument, published, {
      repository, siteUrl: process.env.RELEASE_SITE_URL || undefined,
    }));
  } else if (command === "docs") {
    warnUndocumented(await docsCoverage());
  } else if (command === "tag") {
    // --no-intel leaves Intel Macs out; it combines with either version choice.
    await tagRelease(root, { ...versionOptions(rest.filter((arg) => arg !== "--no-intel")), intel: !rest.includes("--no-intel") });
  } else if (command === "bump" && argument) {
    // Changes the working tree and prints only the tag; warnings go to stderr.
    console.log(await bumpRelease(root, { date: argument, ...versionOptions(rest.slice(1)) }));
  } else {
    throw new Error("Usage: node scripts/release.mjs validate TAG | prepare-assets INPUT_DIR OUTPUT_DIR VERSION | notes TAG PUBLISHED_TAGS_FILE | docs | tag [--no-intel] [--major | VERSION] | bump YYYY-MM-DD [--major | VERSION]");
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
