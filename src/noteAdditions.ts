import { escapeMarkdownText, markdownEntityLink, type NoteReference } from "./noteEntities";

/**
 * The Markdown Add to Note appends: a quoted passage with a source line, or
 * a link to the conversation. The native add (`src-tauri/src/note_additions.rs`)
 * writes the real note; this mirror serves the browser demo and the dialog's
 * preview, and the tests keep the two in step.
 */

/** `text` as a block quote: every line prefixed, no line able to open a code fence inside it. */
export function formatQuote(text: string): string {
  const lines = text.replace(/\r\n?/g, "\n").replace(/^\n+|\n+$/g, "").split("\n");
  return lines.map((raw) => {
    const line = raw.replace(/\s+$/, "");
    if (!line) return ">";
    return /^\s*(```|~~~)/.test(line) ? `> ${line.replace(/^(\s*)/, "$1\\")}` : `> ${line}`;
  }).join("\n") + "\n";
}

/** The block one addition writes, or null when there is nothing to write. */
export function composeAddition(input: { quote: string | null; subject: string; reference: NoteReference; dateLabel: string; linkPresent: boolean; files?: { id: string; name: string; image: boolean }[] }): string | null {
  const link = markdownEntityLink(input.subject.trim() || "(No subject)", input.reference);
  let block = "";
  if (input.quote?.trim()) {
    const date = input.dateLabel.trim();
    block = formatQuote(input.quote) + `> — ${link}${date ? ` · ${escapeMarkdownText(date)}` : ""}\n`;
  } else if (!input.linkPresent) block = `- ${link}\n`;
  const files = input.files ?? [];
  if (files.length) block += `${block ? "\n" : ""}${files.map((file) => `${file.image ? "!" : ""}[${escapeMarkdownText(file.name)}](attachment:${file.id})`).join(" ")}\n`;
  return block || null;
}

/** `previous` with `block` at its end; anything but a bullet follows a blank line. */
export function appendBlock(previous: string, block: string): string {
  if (!previous) return block;
  let body = previous.endsWith("\n") ? previous : `${previous}\n`;
  if (!block.startsWith("- ") && !body.endsWith("\n\n")) body += "\n";
  return body + block;
}
