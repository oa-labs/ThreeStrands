/**
 * Every limit of Add to Note, in one place. The quote and file limits
 * mirror `src-tauri/src/note_additions.rs`, which enforces them natively;
 * the dialog checks them first so it can say why instead of failing on
 * submit. Nothing here is clamped: a value over a limit is refused.
 */
/** How far ahead the note picker looks for meetings with a participant of the conversation. */
export const ADD_TO_NOTE_MEETING_DAYS = 7;
/** Most meetings offered as note suggestions. */
export const MAX_MEETING_NOTE_SUGGESTIONS = 3;
/** Most recently opened notes offered before the user types. */
export const MAX_RECENT_NOTE_SUGGESTIONS = 6;
/** Most notes a title search offers. */
export const MAX_NOTE_SEARCH_SUGGESTIONS = 8;
/** Longest title a note created from the picker can have, as natively. */
export const MAX_NEW_NOTE_TITLE_CHARS = 200;
/** Most characters of quoted text one addition holds (`MAX_ADDITION_QUOTE_CHARS`). */
export const MAX_ADDITION_QUOTE_CHARS = 20_000;
/** Most files one addition copies (`MAX_ADDITION_FILES`). */
export const MAX_ADDITION_FILES = 20;
/** The largest file a note can hold, as natively (`MAX_NOTE_ATTACHMENT_BYTES`). */
export const MAX_NOTE_ATTACHMENT_BYTES = 20 * 1024 * 1024;

/** Whether `text` is within the quote limit, counting characters as natively (code points). */
export const quoteWithinLimit = (text: string): boolean => [...text].length <= MAX_ADDITION_QUOTE_CHARS;
/** Whether a file of `bytes` fits in a note. */
export const fileFitsNote = (bytes: number): boolean => bytes <= MAX_NOTE_ATTACHMENT_BYTES;
