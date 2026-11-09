import type { Note } from "./domain";
import type { DeviceRosterEntry, FrontierCandidate, FrontierConflict } from "./replicatedSync";

/** The fields a note version shows. */
const NOTE_FIELDS = ["title", "body", "trashed"] as const;
type NoteField = (typeof NOTE_FIELDS)[number];

/** One device's version of a conflicted note: its value for each field in
 * conflict, and the shown value for the rest. */
export type NoteVersion = {
  deviceId: string;
  title: string;
  body: string;
  trashed: boolean;
};

export type NoteConflictView = {
  /** The device whose write is shown, when the shown values came from one device. */
  shownDeviceId: string | null;
  others: NoteVersion[];
};

/** A device's latest value for a field, when it has one in the conflict. */
const fromDevice = (conflict: FrontierConflict, deviceId: string) => [...conflict.candidates].reverse().find((candidate) => candidate.deviceId === deviceId);

const isNoteField = (field: string): field is NoteField => (NOTE_FIELDS as readonly string[]).includes(field);

/** Field conflicts on notes, grouped by note id. Conflicts on fields a note
 * version doesn't show are left to Settings. */
export function noteConflictsById(conflicts: FrontierConflict[]): Map<string, FrontierConflict[]> {
  const byNote = new Map<string, FrontierConflict[]>();
  for (const conflict of conflicts) {
    if (conflict.entityType !== "note" || !isNoteField(conflict.field)) continue;
    byNote.set(conflict.entityId, [...(byNote.get(conflict.entityId) ?? []), conflict]);
  }
  return byNote;
}

const sameValue = (field: NoteField, candidate: FrontierCandidate, note: Note) => {
  const value = candidate.value ?? (field === "trashed" ? false : "");
  return value === note[field];
};

/** The candidate holding the value the note shows now, which every device
 * shows the same way; the first candidate if none matches. */
export function shownCandidate(conflict: FrontierConflict, note: Note): FrontierCandidate | undefined {
  const field = conflict.field as NoteField;
  return conflict.candidates.find((candidate) => sameValue(field, candidate, note)) ?? conflict.candidates[0];
}

/** Each device's version of the note that differs from the one shown. */
export function noteConflictView(note: Note, conflicts: FrontierConflict[]): NoteConflictView {
  const shown = conflicts.map((conflict) => shownCandidate(conflict, note)?.deviceId);
  const shownDevices = new Set(shown.filter(Boolean));
  const devices: string[] = [];
  for (const conflict of conflicts) {
    const current = shownCandidate(conflict, note);
    for (const candidate of conflict.candidates) {
      if (candidate !== current && !devices.includes(candidate.deviceId)) devices.push(candidate.deviceId);
    }
  }
  const others = devices.map((deviceId): NoteVersion => {
    const version: NoteVersion = { deviceId, title: note.title, body: note.body, trashed: note.trashed };
    for (const conflict of conflicts) {
      const field = conflict.field as NoteField;
      const candidate = fromDevice(conflict, deviceId);
      if (!candidate) continue;
      const value = candidate.value ?? (field === "trashed" ? false : "");
      if (field === "trashed") version.trashed = value === true;
      else version[field] = typeof value === "string" ? value : "";
    }
    return version;
  });
  return { shownDeviceId: shownDevices.size === 1 ? [...shownDevices][0]! : null, others };
}

/** For each conflicted field, the candidate that keeps `deviceId`'s version,
 * or the shown value where that device didn't write the field. With no
 * device, the shown values. */
export function conflictChoices(note: Note, conflicts: FrontierConflict[], deviceId: string | null): { conflict: FrontierConflict; chosen: FrontierCandidate }[] {
  return conflicts.flatMap((conflict) => {
    const chosen = (deviceId === null ? undefined : fromDevice(conflict, deviceId)) ?? shownCandidate(conflict, note);
    return chosen ? [{ conflict, chosen }] : [];
  });
}

/** A device's name as Settings → Replicated Sync shows it. */
export function deviceName(deviceId: string | null, roster: DeviceRosterEntry[]): string {
  const device = roster.find((entry) => entry.deviceId === deviceId);
  if (device?.label) return device.label;
  if (device?.isSelf) return "this device";
  return "another device";
}

export type DiffLine = { kind: "same" | "removed" | "added"; text: string };

/** Above this many line pairs the comparison marks everything after the
 * shared start and end as changed rather than aligning lines. */
export const MAX_DIFF_CELLS = 4_000_000;

/** A line-by-line comparison of two texts: lines only in `before` are
 * `removed`, lines only in `after` are `added`. */
export function diffLines(before: string, after: string): DiffLine[] {
  const a = before.split("\n");
  const b = after.split("\n");
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let end = 0;
  while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end++;
  const head = a.slice(0, start).map((text): DiffLine => ({ kind: "same", text }));
  const tail = a.slice(a.length - end).map((text): DiffLine => ({ kind: "same", text }));
  const x = a.slice(start, a.length - end);
  const y = b.slice(start, b.length - end);
  if (x.length * y.length > MAX_DIFF_CELLS) {
    return [...head, ...x.map((text): DiffLine => ({ kind: "removed", text })), ...y.map((text): DiffLine => ({ kind: "added", text })), ...tail];
  }
  // Longest common subsequence, filled from the end so the walk runs forward.
  const width = y.length + 1;
  const lengths = new Uint32Array((x.length + 1) * width);
  for (let i = x.length - 1; i >= 0; i--) {
    for (let j = y.length - 1; j >= 0; j--) {
      lengths[i * width + j] = x[i] === y[j] ? lengths[(i + 1) * width + j + 1]! + 1 : Math.max(lengths[(i + 1) * width + j]!, lengths[i * width + j + 1]!);
    }
  }
  const middle: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < x.length && j < y.length) {
    if (x[i] === y[j]) { middle.push({ kind: "same", text: x[i]! }); i++; j++; }
    else if (lengths[(i + 1) * width + j]! >= lengths[i * width + j + 1]!) middle.push({ kind: "removed", text: x[i++]! });
    else middle.push({ kind: "added", text: y[j++]! });
  }
  while (i < x.length) middle.push({ kind: "removed", text: x[i++]! });
  while (j < y.length) middle.push({ kind: "added", text: y[j++]! });
  return [...head, ...middle, ...tail];
}
