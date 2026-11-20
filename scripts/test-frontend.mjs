import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { availableParallelism, homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

// Pin the distribution and its bytes. This canary contains the jsdom and
// Vitest compatibility used by the suite (Bun revision 7bb427f3b).
const version = "1.4.5-canary.20261120.1";
const distributions = {
  "darwin-arm64": ["darwin-aarch64", "Hif4uT/VQH8H3s34XkP8BeZ11M3NYE0eIdHughcYRgoCxkojrDkY+Ecu7F4e7GyMzD/vBscwoF7ZASMHg7LkpQ=="],
  "darwin-x64": ["darwin-x64-baseline", "uzzHRtaWqwFIUagHd/OtaMQWiiWVAEAShCog0j6mT6rXb0G6rbpycY+Is3cYm3z8HA7zG6Ekcq3Zj+y+bbmCiA=="],
  "linux-arm64": ["linux-aarch64", "SrxL7YlYt+hKr6AnKEWKR/dmcnJppx+sn3fDkTRm2JOijKjlFTV3hC5aLwwxpbCn+SUtB3bs8qejWTpJBifAMg=="],
  "linux-x64": ["linux-x64-baseline", "5CQaSBrQMDM81hGJ+f51Op+7UE4QKhM3j2C2JWLJx013inzxVE4Uask5Vz0PAew9p64XmilAJTPHPt/Efs3/Jg=="],
  "win32-arm64": ["windows-aarch64", "NwQ/rrE25m1/yBPvDv0VKrCNTajxGi3az1Fvk+x/TBHlQ3NL6gQzbK2vB1ZVucVSI05xtKmaUWfrt8UwT+3YIQ=="],
  "win32-x64": ["windows-x64-baseline", "YYF4YLcsZUhvorM8oOV2ZfdgV0E3BdpM0BGW7gwFnbQAJBQENQhofwbCUoVa7z+GQU3GihgK9WGqJ47ktTXdAQ=="],
};

async function testBinary() {
  const platform = `${process.platform}-${process.arch}`;
  const distribution = distributions[platform];
  if (!distribution) throw new Error(`No pinned Bun binary for ${platform}.`);

  const [suffix, checksum] = distribution;
  const binary = process.platform === "win32" ? "bun.exe" : "bun";
  const cacheRoot = (process.platform === "win32" ? process.env.LOCALAPPDATA : process.env.XDG_CACHE_HOME)
    || join(homedir(), ".cache");
  const cache = join(cacheRoot, "threestrands", `bun-${version}-${suffix}`);
  const executable = join(cache, binary);
  if (existsSync(executable)) return executable;

  const packageName = `bun-${suffix}`;
  const url = `https://registry.npmjs.org/@oven/${packageName}/-/${packageName}-${version}.tgz`;
  console.error(`Downloading pinned Bun ${version} for ${platform}…`);
  const response = await fetch(url, { signal: AbortSignal.timeout(120_000) });
  if (!response.ok) throw new Error(`Bun download failed (HTTP ${response.status}).`);
  const archive = Buffer.from(await response.arrayBuffer());
  if (createHash("sha512").update(archive).digest("base64") !== checksum) {
    throw new Error(`Checksum mismatch for ${packageName}@${version}.`);
  }

  mkdirSync(cache, { recursive: true });
  const staging = mkdtempSync(join(cache, "download-"));
  try {
    const archivePath = join(staging, "bun.tgz");
    writeFileSync(archivePath, archive);
    const tar = spawnSync("tar", ["-xzf", archivePath, "-C", staging, `package/bin/${binary}`], { stdio: "inherit" });
    if (tar.error) throw tar.error;
    if (tar.status !== 0) throw new Error(`Could not extract ${packageName}@${version}.`);
    try {
      renameSync(join(staging, "package", "bin", binary), executable);
    } catch (error) {
      // A concurrent run may already have published the verified binary.
      if (!existsSync(executable)) throw error;
    }
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
  return executable;
}

try {
  const cores = availableParallelism();
  // Leave room for other agents and the developer. CI runners are dedicated.
  const workers = process.env.CI ? cores : Math.max(1, Math.floor(cores / 2));
  const result = spawnSync(await testBinary(), [
    "test", `--parallel=${workers}`, "--isolate", "--timeout=20000", "--dots",
    "--define", "__DEMO_CLIENT__:true", ...process.argv.slice(2),
  ], {
    cwd: fileURLToPath(new URL("../", import.meta.url)),
    env: {
      ...process.env,
      TZ: "America/New_York",
      LANG: "en_US.UTF-8",
      LC_ALL: "en_US.UTF-8",
      RTL_SKIP_AUTO_CLEANUP: "true",
      DO_NOT_TRACK: "1",
    },
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
