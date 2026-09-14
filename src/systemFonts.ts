import { invoke } from "@tauri-apps/api/core";

let cachedFamilies: Promise<string[]> | null = null;

function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export function listSystemFontFamilies(): Promise<string[]> {
  if (!isTauri()) return Promise.resolve([]);
  if (!cachedFamilies) {
    cachedFamilies = invoke<string[]>("list_system_font_families").catch((error) => {
      cachedFamilies = null;
      throw error;
    });
  }
  return cachedFamilies;
}
