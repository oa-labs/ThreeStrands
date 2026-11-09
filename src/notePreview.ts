import { splitWikiLink } from "./noteWikiLinks";

/** Most characters of a note shown under its title in the list, as natively. */
export const NOTE_PREVIEW_CHARS = 140;

const isFence = (line: string) => /^ {0,3}(`{3,}|~{3,})/.test(line);
const isRule = (line: string) => {
  const marks = line.replace(/\s/g, "");
  return marks.length >= 3 && /^(-+|\*+|_+)$/.test(marks);
};
const isTableDivider = (line: string) => line.includes("-") && line.includes("|") && /^[|\-: ]+$/.test(line);

function stripBlockMarkers(line: string): string {
  for (;;) {
    const before = line;
    line = line.trimStart().replace(/^>/, "").trimStart();
    line = line.replace(/^#{1,6}(?= )/, "").trimStart();
    line = line.replace(/^[-*+] /, "").replace(/^\d+[.)] /, "").replace(/^\[[ xX]\] /, "");
    if (line === before) return line;
  }
}

function inlinePlain(line: string): string {
  const chars = [...line];
  let out = "";
  let index = 0;
  const run = (ch: string) => { let count = 0; while (chars[index + count] === ch) count++; return count; };
  while (index < chars.length) {
    const ch = chars[index]!;
    const next = chars[index + 1];
    if (ch === "\\" && next !== undefined && /[!-/:-@[-`{-~]/.test(next)) { out += next; index += 2; continue; }
    if (ch === "[" && next === "[") {
      const close = chars.indexOf("]", index + 2);
      const inner = close >= 0 && chars[close + 1] === "]" ? chars.slice(index + 2, close).join("") : null;
      if (inner !== null && !inner.includes("[")) {
        const { target, label } = splitWikiLink(inner);
        if (target) { out += label; index = close + 2; continue; }
      }
      out += ch; index++; continue;
    }
    if (ch === "!" && next === "[") { index++; continue; }
    if (ch === "[") {
      const close = chars.indexOf("]", index + 1);
      const end = close >= 0 && chars[close + 1] === "(" ? chars.indexOf(")", close + 2) : -1;
      if (end >= 0) { out += inlinePlain(chars.slice(index + 1, close).join("")); index = end + 1; continue; }
      out += ch; index++; continue;
    }
    if (ch === "<") {
      const end = chars.indexOf(">", index);
      const inside = chars.slice(index + 1, end < 0 ? chars.length : end).join("");
      if (inside.includes("://")) { out += inside; index = end < 0 ? chars.length : end + 1; continue; }
    }
    if ((ch === "*" || ch === "`") || ((ch === "~" || ch === "=") && next === ch)) { index += run(ch); continue; }
    if (ch === "_") {
      const count = run("_");
      const before = chars[index - 1];
      const after = chars[index + count];
      // Underscores mark emphasis at word edges; inside a word (snake_case) they're text.
      if (before && after && /[\p{L}\p{N}]/u.test(before) && /[\p{L}\p{N}]/u.test(after)) out += "_".repeat(count);
      index += count;
      continue;
    }
    out += ch === "|" ? " " : ch;
    index++;
  }
  return out;
}

/** A note's Markdown as the plain text the list shows, by the native rules (src-tauri/src/notes.rs `plain_text`). */
export function notePlainText(body: string): string {
  const lines = body.split(/\r?\n/);
  let start = 0;
  if (lines[0]?.trimEnd() === "---") {
    start = 1;
    while (start < lines.length && !["---", "..."].includes(lines[start]!.trimEnd())) start++;
    start++;
  }
  const words: string[] = [];
  for (const raw of lines.slice(start)) {
    const line = raw.trim();
    if (isFence(line) || isRule(line) || isTableDivider(line)) continue;
    words.push(...inlinePlain(stripBlockMarkers(line)).split(/\s+/).filter(Boolean));
  }
  return words.join(" ");
}

/** The list's preview: plain text, cut with an ellipsis. */
export function notePreview(body: string): string {
  const text = notePlainText(body);
  return [...text].length <= NOTE_PREVIEW_CHARS ? text : `${[...text].slice(0, NOTE_PREVIEW_CHARS - 1).join("")}…`;
}
