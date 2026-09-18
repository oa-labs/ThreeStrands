import { spawnSync } from "node:child_process";
import { delimiter, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const env = { ...process.env };

// Tauri 2.11.4 inherits a close-on-exec stdin descriptor when its Node CLI
// launches actool, which makes Icon Composer compilation fail. Keep this
// macOS-only shim until https://github.com/tauri-apps/tauri/pull/15991 ships.
if (process.platform === "darwin") {
  env.PATH = `${resolve(root, "scripts/tauri-bin")}${delimiter}${env.PATH ?? ""}`;
}

const result = spawnSync(
  process.execPath,
  [resolve(root, "node_modules/@tauri-apps/cli/tauri.js"), ...process.argv.slice(2)],
  { env, stdio: "inherit" },
);

if (result.error) throw result.error;
process.exit(result.status ?? 1);
