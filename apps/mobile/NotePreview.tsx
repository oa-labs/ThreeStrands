import DOMPurify from "dompurify";
import { Marked, type Token, type Tokens } from "marked";
import { useEffect, useMemo, useRef } from "react";
import { splitWikiLink } from "../../src/noteWikiLinks";
import { noteLinkTarget } from "../../src/noteLinks";
import { parseNoteReference, type NoteItem } from "../../src/noteEntities";
import { MEETINGS_HEADING } from "../../src/noteMeetings";

const escaped = (text: string) => text.replace(/[&<>"']/g, char => ({
  "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
})[char]!);

// Generated entries start with an event reference and use typed contact
// references for attendees. Ordinary prose and nested notes keep their layout.
function meetingParts(tokens: Token[]) {
  const [time, ...tail] = tokens;
  if (time?.type !== "link" || parseNoteReference(time.href)?.kind !== "event") return;
  const attendees: Token[][] = [];
  let title: Token[] = [];
  for (let index = 0; index < tail.length; index++) {
    const token = tail[index];
    if (token.type === "text" && token.raw.startsWith(" for ")) {
      const text = token.text.slice(5);
      title = [...(text ? [{ ...token, text, raw: token.raw.slice(5) }] : []), ...tail.slice(index + 1)];
      break;
    }
    if (token.type !== "text" || token.raw !== (attendees.length ? ", " : " met with ")) return;
    const contact = tail[++index];
    if (contact?.type !== "link" || parseNoteReference(contact.href)?.kind !== "contact") return;
    attendees.push(contact.tokens ?? []);
  }
  if (title.length) return { time: time.tokens ?? [], attendees, title };
}

export function renderPreview(body: string, items: NoteItem[], disabled: boolean, focused?: string, daily = false) {
  let markerPrefix = "THREESTRANDS_NOTE_ITEM_";
  while (body.includes(markerPrefix)) markerPrefix += "_";
  const expression = new RegExp(`^${markerPrefix}(\\d+)\\b`);
  const bytes = new TextEncoder().encode(body);
  const decode = new TextDecoder("utf-8", { ignoreBOM: true });
  const parts: string[] = [];
  let offset = 0;
  for (const [index, item] of items.entries()) {
    parts.push(decode.decode(bytes.subarray(offset, item.markerStart)), `${markerPrefix}${index}`);
    offset = item.markerEnd;
  }
  parts.push(decode.decode(bytes.subarray(offset)));

  const meetings = new Set<Tokens.ListItem>();
  const markdown = new Marked({
    hooks: {
      processAllTokens(tokens) {
        if (!daily) return tokens;
        let inMeetings = false;
        for (const token of tokens) {
          if (token.type === "heading" && token.depth <= 2) {
            inMeetings = token.depth === 2 && token.text.toLowerCase() === MEETINGS_HEADING.toLowerCase();
          }
          if (inMeetings && token.type === "list" && !token.ordered) {
            for (const item of token.items) meetings.add(item);
          }
        }
        return tokens;
      },
    },
    renderer: {
      listitem(item) {
        const [first, ...rest] = item.tokens;
        if (!first || (first.type !== "text" && first.type !== "paragraph") || !first.tokens) return false;
        const [marker, ...label] = first.tokens;
        if (marker?.type === "noteItem") {
          return `<li class="note-check-item"><div class="note-check-row">${this.parser.parseInline([marker])}<div>${this.parser.parseInline(label).trimStart()}${this.parser.parse(rest)}</div></div></li>`;
        }
        if (!meetings.has(item) || first.text.includes("\n")) return false;
        const meeting = meetingParts(first.tokens);
        if (!meeting) return false;
        const title = this.parser.parseInline(meeting.title);
        const attendees = meeting.attendees.map(person => this.parser.parseInline(person));
        const metadata = [this.parser.parseInline(meeting.time), ...attendees].join(" · ");
        return `<li class="meeting-item"><div class="meeting-title">${title}</div><div class="meeting-meta">${metadata}</div>${this.parser.parse(rest)}</li>`;
      },
      html: ({ text }) => text.trimStart().startsWith("<!--") ? "" : escaped(text),
      link({ href, tokens }) {
        const target = noteLinkTarget(href);
        const label = this.parser.parseInline(tokens);
        return target ? `<button type="button" class="inline-link" data-external="${escaped(target)}">${label}</button>` : label;
      },
      image: ({ text }) => `<span class="muted">${escaped(text || "Image")}</span>`,
    },
    extensions: [{
      name: "noteItem", level: "inline",
      start: source => source.indexOf(markerPrefix),
      tokenizer(source) {
        const match = expression.exec(source);
        if (match) return { type: "noteItem", raw: match[0], index: Number(match[1]) };
      },
      renderer(token) {
        const item = items[token.index as number];
        if (!item) return "";
        return `<button type="button" class="note-check ${item.task ? "round" : "square"}${item.id === focused ? " focused-task" : ""}"
          data-note-item="${escaped(item.id)}" role="checkbox" aria-checked="${item.checked}"
          aria-label="${escaped(item.title)}"${disabled ? " disabled" : ""}><span class="check-mark">${item.checked ? "✓" : ""}</span></button>`;
      },
    }, {
      name: "wikiLink", level: "inline",
      start: source => source.indexOf("[["),
      tokenizer(source) {
        const match = /^\[\[([^\][\n]+)\]\]/.exec(source);
        if (match) return { type: "wikiLink", raw: match[0], ...splitWikiLink(match[1]) };
      },
      renderer(token) {
        const { target, label } = token as Tokens.Generic & { target: string; label: string };
        return target ? `<button type="button" class="inline-link" data-wiki="${escaped(target)}">${escaped(label)}</button>` : escaped(label);
      },
    }],
  });
  return DOMPurify.sanitize(markdown.parse(parts.join(""), { async: false }), {
    ALLOWED_TAGS: ["div", "p", "br", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "blockquote", "strong", "em", "del", "hr", "pre", "code", "span", "button", "table", "thead", "tbody", "tr", "td", "th"],
    ALLOWED_ATTR: ["class", "start", "type", "role", "aria-checked", "aria-label", "disabled", "data-note-item", "data-wiki", "data-external"],
    ALLOW_DATA_ATTR: false,
  });
}

export function NotePreview({ body, items, disabled, focused, daily, onCheck, onWiki, onUrl }: {
  body: string; items: NoteItem[]; disabled: boolean; focused?: string; daily?: boolean;
  onCheck(item: NoteItem): void; onWiki(title: string): void; onUrl(url: string): void;
}) {
  const html = useMemo(() => renderPreview(body, items, disabled, focused, daily), [body, items, disabled, focused, daily]);
  const preview = useRef<HTMLDivElement>(null);
  const revealed = useRef<string>(undefined);
  useEffect(() => {
    if (!focused || revealed.current === focused) return;
    const task = preview.current?.querySelector(".focused-task");
    if (!task) return;
    revealed.current = focused;
    task.scrollIntoView?.({ block: "center" });
  }, [focused, html]);

  return <div className="note-preview" ref={preview}
    onClick={event => {
      const button = (event.target as Element).closest<HTMLButtonElement>("button");
      if (!button || button.disabled) return;
      const { noteItem, wiki, external } = button.dataset;
      if (wiki) onWiki(wiki);
      else if (external) onUrl(external);
      else if (noteItem) {
        const item = items.find(value => value.id === noteItem);
        if (item) onCheck(item);
      }
    }}
    dangerouslySetInnerHTML={{ __html: html }}
  />;
}
