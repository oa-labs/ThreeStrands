import { useSyncExternalStore } from "react";

/** Mirrors `MAX_RECAP_INSTRUCTIONS_CHARS` in `src-tauri/src/recording/policy.rs`, counted in characters. */
export const MAX_MEETING_SUMMARY_INSTRUCTIONS_CHARS = 2000;

export const MEETING_SUMMARY_INSTRUCTIONS_KEY = "threestrands.meetingSummaryInstructions";

export type MeetingSummaryPreset = {
  id: string;
  label: string;
  instructions: string;
};

/** Starter templates users can pick or customize for AI meeting summaries. */
export const MEETING_SUMMARY_PRESETS: readonly MeetingSummaryPreset[] = [
  {
    id: "default",
    label: "Default",
    instructions: "",
  },
  {
    id: "executive",
    label: "Executive Brief",
    instructions:
      "Lead with a crisp 2-sentence bottom-line outcome, then group into concise bold topic headings covering strategic decisions, risks/trade-offs, and key commitments. Keep bullets tight and scannable.",
  },
  {
    id: "detailed",
    label: "Detailed Discussion",
    instructions:
      "Provide a brief overview followed by a thorough topic-by-topic breakdown. Under each bold topic heading, capture context, options considered, key arguments, and important specifics.",
  },
  {
    id: "oneOnOne",
    label: "1:1 & Coaching",
    instructions:
      "Organize around progress & wins, blockers or concerns raised, feedback & coaching themes, and career or priority check-ins. Keep the tone constructive and specific.",
  },
];

const listeners = new Set<() => void>();
let snapshot: string | null = null;

function withinLimit(value: string, limit: number): boolean {
  return [...value].length <= limit;
}

/**
 * Normalizes synced or imported summary instructions. Non-strings, whitespace-only
 * text, or values exceeding `MAX_MEETING_SUMMARY_INSTRUCTIONS_CHARS` fall back to empty.
 */
export function sanitizeMeetingSummaryInstructions(value: unknown): string {
  if (typeof value !== "string") return "";
  if (!value.trim()) return "";
  if (!withinLimit(value, MAX_MEETING_SUMMARY_INSTRUCTIONS_CHARS)) return "";
  return value.replace(/\r\n/g, "\n");
}

export function readMeetingSummaryInstructions(): string {
  try {
    return sanitizeMeetingSummaryInstructions(localStorage.getItem(MEETING_SUMMARY_INSTRUCTIONS_KEY));
  } catch {
    return "";
  }
}

/** Replaces the stored summary instructions (used by Settings, transfer import, and sync). */
export function saveMeetingSummaryInstructions(value: unknown): void {
  snapshot = sanitizeMeetingSummaryInstructions(value);
  try {
    if (snapshot) localStorage.setItem(MEETING_SUMMARY_INSTRUCTIONS_KEY, snapshot);
    else localStorage.removeItem(MEETING_SUMMARY_INSTRUCTIONS_KEY);
  } catch {
    // Instructions still apply in memory for this session when storage is unavailable.
  }
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function getSnapshot(): string {
  return snapshot ??= readMeetingSummaryInstructions();
}

/** Re-reads storage on next access; for tests that seed or clear it directly. */
export function resetMeetingSummaryInstructionsForTests(): void {
  snapshot = null;
  listeners.forEach((listener) => listener());
}

/** Synced meeting summary instructions shared across Settings and summarization callers. */
export function useMeetingSummaryInstructions(): string {
  return useSyncExternalStore(subscribe, getSnapshot);
}
