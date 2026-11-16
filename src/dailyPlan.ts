import { fenceRun, indent, isListItem, itemEnd, lineEnd, lines as splitLines, type Line } from "./meetingExcerpt";
import { escapeMarkdownText, extractNoteEntityLinks, noteItemRanges, noteReferenceHref, type NoteReference } from "./noteEntities";
import { noteInlineText } from "./notePreview";

/** Mirrors `src-tauri/src/daily_plan.rs` for the demo client; both are held to
 * `fixtures/daily-plan.json`. */
export const PLAN_HEADING = "Plan";
export const NOT_TODAY_HEADING = "Not today";
const MARKER = "<!-- threestrands-plan:";

export type PlanItemKind = "task" | "noteTask" | "newTask" | "contact" | "text";
export type PlanMarker = { kind: "carried" | "deferred"; date: string } | { kind: "dropped" };
export type PlanLine = { kind: PlanItemKind; reference: NoteReference | null; label: string; markerId: string | null; checked: boolean; marker: PlanMarker | null; start: number; end: number };
export type PlanSection = { start: number; end: number; items: PlanLine[]; notToday: PlanLine[] };
export type PlanAddition = { kind: "link"; reference: NoteReference; label: string } | { kind: "newTask"; title: string };

const isDate = (value: string) => /^\d{4}-\d{2}-\d{2}$/.test(value) && !Number.isNaN(Date.parse(`${value}T00:00:00Z`));

function heading(text: string): { level: number; key: string } | null {
  const match = /^( {0,3})(#{1,6})(?:[ \t](.*))?$/.exec(text);
  if (!match) return null;
  let content = (match[3] ?? "").trim();
  const without = content.replace(/#+$/, "");
  if (without.length < content.length && (!without || /[ \t]$/.test(without))) content = without.trimEnd();
  return { level: match[2].length, key: content.split(/\s+/).filter(Boolean).join(" ").toLowerCase() };
}

function scan(all: Line[]): { code: boolean; heading: ReturnType<typeof heading> }[] {
  let fence: { mark: string; run: number } | null = null;
  return all.map((line) => {
    const run = fenceRun(line.text);
    if (run) {
      if (fence) {
        if (fence.mark === run.mark && run.run >= fence.run && !run.trailing) fence = null;
        return { code: true, heading: null };
      }
      if (indent(line.text) <= 3) { fence = { mark: run.mark, run: run.run }; return { code: true, heading: null }; }
    }
    return fence ? { code: true, heading: null } : { code: false, heading: heading(line.text) };
  });
}

function marker(text: string): PlanMarker | null {
  const at = text.lastIndexOf(MARKER);
  if (at < 0 || !text.endsWith(" -->")) return null;
  const rest = text.slice(at + MARKER.length, -4);
  const [kind, date, ...extra] = rest.split(" ");
  if (extra.length) return null;
  if ((kind === "carried" || kind === "deferred") && date !== undefined && isDate(date)) return { kind, date };
  return kind === "dropped" && date === undefined ? { kind: "dropped" } : null;
}

/** The plan section of `body`, or null without a `## Plan` heading outside code. */
export function parsePlan(noteId: string, body: string): PlanSection | null {
  const all = splitLines(body);
  const scanned = scan(all);
  const head = scanned.findIndex((line) => line.heading?.level === 2 && line.heading.key === PLAN_HEADING.toLowerCase());
  if (head < 0) return null;
  const closeAt = scanned.slice(head + 1).findIndex((line) => line.heading && line.heading.level <= 2);
  const close = closeAt < 0 ? all.length : head + 1 + closeAt;
  const notTodayAt = scanned.slice(head + 1, close).findIndex((line) => line.heading?.level === 3 && line.heading.key === NOT_TODAY_HEADING.toLowerCase());
  const notToday = notTodayAt < 0 ? null : head + 1 + notTodayAt;
  const notTodayClose = notToday === null ? null : (() => { const at = scanned.slice(notToday + 1, close).findIndex((line) => line.heading && line.heading.level <= 3); return at < 0 ? close : notToday + 1 + at; })();
  const links = extractNoteEntityLinks(body).links;
  const items = noteItemRanges(body);
  const collect = (from: number, to: number, skip: [number, number] | null): PlanLine[] => {
    const out: PlanLine[] = [];
    let at = from;
    while (at < to) {
      if (skip && at >= skip[0] && at < skip[1]) { at = skip[1]; continue; }
      const line = all[at];
      if (scanned[at].code || scanned[at].heading || line.text.trimStart().startsWith(">") || !isListItem(line.text) || indent(line.text) > 3) { at++; continue; }
      const end = Math.min(itemEnd(all, at) ?? lineEnd(line), lineEnd(all[to - 1]));
      const link = links.find((candidate) => candidate.start >= line.start && candidate.start < lineEnd(line));
      const item = items.find((candidate) => candidate.start === line.start);
      let kind: PlanItemKind = "text", reference: NoteReference | null = null, label: string;
      if (link && link.reference.kind === "task") { kind = link.reference.note ? "noteTask" : "task"; reference = link.reference; label = link.label; }
      else if (link && link.reference.kind === "contact") { kind = "contact"; reference = link.reference; label = link.label; }
      else if (item?.task) {
        kind = "newTask";
        label = noteInlineText(body.slice(item.textStart, item.end).replace(/<!--.*?-->/g, "").trim());
        reference = item.id ? { version: 1, kind: "task", id: item.id, note: noteId } : null;
      } else {
        const text = line.text.trimStart().replace(/^\S+[ \t]+/, "");
        const at = text.indexOf(MARKER);
        label = (at < 0 ? text : text.slice(0, at)).trim();
      }
      out.push({ kind, reference, label, markerId: item?.task ? item.id ?? null : null, checked: Boolean(item?.checked), marker: marker(line.text), start: line.start, end });
      while (at < to && all[at].start < end) at++;
    }
    return out;
  };
  return {
    start: all[head].start,
    end: lineEnd(all[close - 1]),
    items: collect(head + 1, close, notToday !== null && notTodayClose !== null ? [notToday, notTodayClose] : null),
    notToday: notToday !== null && notTodayClose !== null ? collect(notToday + 1, notTodayClose, null) : [],
  };
}

/** The Markdown list line for an addition, with an optional decision marker. */
export function planLine(addition: PlanAddition, mark: PlanMarker | null = null): string {
  let line: string;
  if (addition.kind === "link") {
    if (addition.reference.kind !== "task" && addition.reference.kind !== "contact") throw new Error("A plan can link only tasks and contacts");
    const label = addition.label.split(/\s+/).filter(Boolean).join(" ");
    line = `- [${escapeMarkdownText(label || addition.reference.id)}](${noteReferenceHref(addition.reference)})`;
  } else {
    const title = addition.title.split(/\s+/).filter(Boolean).join(" ");
    if (!title) throw new Error("A new task needs a title");
    line = `- ( ) ${escapeMarkdownText(title)}`;
  }
  if (mark?.kind === "dropped") line += ` ${MARKER}dropped -->`;
  else if (mark) line += ` ${MARKER}${mark.kind} ${mark.date} -->`;
  return line;
}

/** `body` with `newLines` added after the existing plan items (or under
 * `### Not today`), creating the section before `## Meetings` or at the end. */
export function insertPlanLines(noteId: string, body: string, newLines: string[], notToday = false): string {
  if (!newLines.length) return body;
  if (extractNoteEntityLinks(body).unclosedFence) throw new Error("unclosed fence");
  const joined = newLines.join("\n");
  const all = splitLines(body);
  const scanned = scan(all);
  const section = parsePlan(noteId, body);
  if (!section) {
    const block = `## ${PLAN_HEADING}\n\n${notToday ? `### ${NOT_TODAY_HEADING}\n\n` : ""}${joined}\n`;
    const meetings = scanned.findIndex((line) => line.heading?.level === 2 && line.heading.key === "meetings");
    if (meetings >= 0) return `${body.slice(0, all[meetings].start)}${block}\n${body.slice(all[meetings].start)}`;
    const separator = !body || body.endsWith("\n\n") ? "" : body.endsWith("\n") ? "\n" : "\n\n";
    return `${body}${separator}${block}`;
  }
  const head = all.findIndex((line) => line.start === section.start);
  const closeAt = all.findIndex((line) => line.start > section.end);
  const close = closeAt < 0 ? all.length : closeAt;
  const notTodayHead = scanned.findIndex((line, at) => at > head && at < close && line.heading?.level === 3 && line.heading.key === NOT_TODAY_HEADING.toLowerCase());
  let at: number;
  if (notToday) {
    if (notTodayHead < 0) {
      const last = all.slice(head, close).reverse().find((line) => line.text.trim());
      const point = last ? lineEnd(last) : lineEnd(all[head]);
      return `${body.slice(0, point)}\n\n### ${NOT_TODAY_HEADING}\n\n${joined}${body.slice(point)}`;
    }
    at = section.notToday.at(-1)?.end ?? lineEnd(all[notTodayHead]);
  } else {
    const last = section.items.at(-1);
    if (!last) { const point = lineEnd(all[head]); return `${body.slice(0, point)}\n\n${joined}${body.slice(point)}`; }
    at = last.end;
  }
  return `${body.slice(0, at)}${notToday && section.notToday.length === 0 ? "\n\n" : "\n"}${joined}${body.slice(at)}`;
}

/** `body` without the item at `start..end` and its line break. */
export function removePlanLine(body: string, start: number, end: number): string {
  const after = body.startsWith("\n", end) ? end + 1 : end;
  const before = after === end && start > 0 && body[start - 1] === "\n" ? start - 1 : start;
  return body.slice(0, before) + body.slice(after);
}
