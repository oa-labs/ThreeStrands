import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

/** Read app CSS with its local imports in the same order Vite bundles them. */
export function readStylesheet(path: string): string {
  return readFileSync(path, "utf8").replace(/^@import "(.+)";$/gm,
    (_, imported: string) => readStylesheet(resolve(dirname(path), imported)));
}
