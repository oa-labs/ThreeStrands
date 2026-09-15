import { invoke } from "@tauri-apps/api/core";

/** `null` means unlimited — mail is never pruned locally. */
export const RETENTION_OPTIONS: { value: number | null; label: string }[] = [
  { value: 30, label: "30 days" },
  { value: 90, label: "90 days" },
  { value: 365, label: "1 year" },
  { value: null, label: "Forever" },
];

function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

// Outside Tauri (browser preview, tests) there's no local database, so this
// only lives in memory for the session.
let demoRetentionDays: number | null = null;

export async function getRetentionDays(): Promise<number | null> {
  if (!isTauri()) return demoRetentionDays;
  return invoke("get_retention_days");
}

export async function setRetentionDays(days: number | null): Promise<void> {
  if (!isTauri()) {
    demoRetentionDays = days;
    return;
  }
  await invoke("set_retention_days", { days });
}
