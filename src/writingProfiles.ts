import { useSyncExternalStore } from "react";

/**
 * What Draft Review knows about the user when they write from one account:
 * who they are, and how they want drafts reviewed. Both are written by the
 * user and sent with every review from that account.
 */
export type WritingProfile = {
  aboutYou: string;
  reviewInstructions: string;
};

/** Writing profile by lowercased mail account email. */
export type WritingProfileMap = Record<string, WritingProfile>;

/** Mirrors `MAX_WRITING_PROFILE_*_CHARS` in `src-tauri/src/ai.rs`, counted in characters. */
export const WRITING_PROFILE_LIMITS = { aboutYou: 2000, reviewInstructions: 2000 } as const;

export const WRITING_PROFILES_KEY = "threestrands.writingProfiles";

const EMPTY_PROFILE: WritingProfile = { aboutYou: "", reviewInstructions: "" };
const listeners = new Set<() => void>();
let snapshot: WritingProfileMap | null = null;

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The key one account's profile is stored under. */
export function writingProfileKey(account: string): string {
  return account.trim().toLowerCase();
}

/** Whether a profile has anything for Draft Review to use. */
export function hasWritingProfile(profile: WritingProfile): boolean {
  return Boolean(profile.aboutYou.trim() || profile.reviewInstructions.trim());
}

function withinLimit(value: string, limit: number): boolean {
  return [...value].length <= limit;
}

/**
 * Keeps string profiles within the limits under lowercased account keys.
 * Empty or over-limit profiles are dropped rather than cut short, matching
 * what the native settings import accepts.
 */
export function sanitizeWritingProfiles(value: unknown): WritingProfileMap {
  const profiles: WritingProfileMap = {};
  if (!isPlainObject(value)) return profiles;
  for (const [account, entry] of Object.entries(value)) {
    const key = writingProfileKey(account);
    if (!key || !isPlainObject(entry)) continue;
    const profile = {
      aboutYou: typeof entry.aboutYou === "string" ? entry.aboutYou : "",
      reviewInstructions: typeof entry.reviewInstructions === "string" ? entry.reviewInstructions : "",
    };
    if (!hasWritingProfile(profile)
      || !withinLimit(profile.aboutYou, WRITING_PROFILE_LIMITS.aboutYou)
      || !withinLimit(profile.reviewInstructions, WRITING_PROFILE_LIMITS.reviewInstructions)) continue;
    profiles[key] = profile;
  }
  return profiles;
}

export function readWritingProfiles(): WritingProfileMap {
  try {
    return sanitizeWritingProfiles(JSON.parse(localStorage.getItem(WRITING_PROFILES_KEY) ?? "{}"));
  } catch {
    // Unreadable or blocked storage reviews every draft without a profile.
    return {};
  }
}

/** Replaces every profile, as when importing settings or applying another device's. */
export function saveWritingProfiles(value: unknown) {
  snapshot = sanitizeWritingProfiles(value);
  try {
    if (Object.keys(snapshot).length) localStorage.setItem(WRITING_PROFILES_KEY, JSON.stringify(snapshot));
    else localStorage.removeItem(WRITING_PROFILES_KEY);
  } catch {
    // The profiles still apply for this session when storage is unavailable.
  }
  listeners.forEach((listener) => listener());
}

/** Sets one account's profile; an empty profile removes it. */
export function setWritingProfile(account: string, profile: WritingProfile) {
  const key = writingProfileKey(account);
  if (!key) return;
  const { [key]: _previous, ...others } = getSnapshot();
  saveWritingProfiles(hasWritingProfile(profile) ? { ...others, [key]: profile } : others);
}

export function writingProfileFor(profiles: WritingProfileMap, account: string): WritingProfile {
  return profiles[writingProfileKey(account)] ?? EMPTY_PROFILE;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function getSnapshot() {
  return snapshot ??= readWritingProfiles();
}

/** Re-reads storage on next access; for tests that seed or clear it directly. */
export function resetWritingProfilesForTests() {
  snapshot = null;
  listeners.forEach((listener) => listener());
}

/** Writing profiles shared by Settings and the composer, so an edit shows in both. */
export function useWritingProfiles(): WritingProfileMap {
  return useSyncExternalStore(subscribe, getSnapshot);
}
