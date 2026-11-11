/** Notes list context survives workspace unmounts within this window's session. */
export type NotesListView = "notes" | "trash";
type NotesSearchState = { query: string; listView: NotesListView; includeTrash: boolean };
const KEY = "threestrands.notes.search";

export function readNotesSearchState(): NotesSearchState {
  try {
    const value = JSON.parse(sessionStorage.getItem(KEY) ?? "null");
    if (value && typeof value.query === "string" && (value.listView === "notes" || value.listView === "trash") && typeof value.includeTrash === "boolean") return value;
  } catch { /* Unavailable or invalid storage starts with active notes. */ }
  return { query: "", listView: "notes", includeTrash: false };
}

export function writeNotesSearchState(value: NotesSearchState): void {
  try { sessionStorage.setItem(KEY, JSON.stringify(value)); } catch { /* Keep the mounted view usable without storage. */ }
}
