import DOMPurify from "dompurify";
import { Marked, type Tokens } from "marked";
import { useMemo } from "react";
import { splitWikiLink } from "../../src/noteWikiLinks";
import { noteLinkTarget } from "../../src/noteLinks";
import type { NoteItem } from "../../src/noteEntities";

const escaped = (text: string) => text.replace(/[&<>"']/g, char => ({
  "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
})[char]!);

export function renderPreview(body: string, items: NoteItem[], disabled: boolean, focused?: string) {
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

  const markdown = new Marked({
    renderer: {
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
          aria-label="${escaped(item.title)}"${disabled ? " disabled" : ""}>${item.checked ? "✓" : ""}</button>`;
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
    ALLOWED_TAGS: ["p", "br", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "blockquote", "strong", "em", "del", "hr", "pre", "code", "span", "button", "table", "thead", "tbody", "tr", "td", "th"],
    ALLOWED_ATTR: ["class", "start", "type", "role", "aria-checked", "aria-label", "disabled", "data-note-item", "data-wiki", "data-external"],
    ALLOW_DATA_ATTR: false,
  });
}

export function NotePreview({ body, items, disabled, focused, onCheck, onWiki, onUrl }: {
  body: string; items: NoteItem[]; disabled: boolean; focused?: string;
  onCheck(item: NoteItem): void; onWiki(title: string): void; onUrl(url: string): void;
}) {
  const html = useMemo(() => renderPreview(body, items, disabled, focused), [body, items, disabled, focused]);
  return <div
    className="note-preview"
    ref={element => {
      if (focused) element?.querySelector(".focused-task")?.scrollIntoView?.({ block: "center" });
    }}
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
