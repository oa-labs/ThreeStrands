import { useEffect, useMemo, useState } from "react";
import { useEditor, useExtension, type WikilinkItem } from "@meowdown/react";
import { defineWikilinkTrigger, type EditorExtension, type TypedEditor } from "@meowdown/core";
import { useEditorDerivedValue } from "@prosekit/react";
import { AutocompleteEmpty, AutocompleteItem, AutocompletePopup, AutocompletePositioner, AutocompleteRoot } from "@prosekit/react/autocomplete";
import { Mail, NotebookText, UserRound } from "lucide-react";
import { ICON_SIZE } from "./iconSizes";
import { errorMessage } from "./errors";
import { markdownEntityLink, noteReferenceHref } from "./noteEntities";
import { noteLinkDetail, type NoteLinkChoice } from "./noteLinkChoices";

// Keep email addresses literal. A standalone @ starts entity search; [[ always searches notes.
export const NOTE_LINK_TRIGGER = /(?:\[\[[^[\]\r\n]*|(?<!\S)@(?:[^\s@[\]][^@[\]\r\n]*)?)$/u;
const queryFromMatch = (match: RegExpMatchArray) => match[0];
const showAll = () => true;

type Choice = { key: string; label: string; detail?: string; markdown: string; kind: "note" | "contact" | "conversation" };
type SearchResult = { query: string; items: Choice[]; error?: string };

function acceptsLinks(editor: TypedEditor): boolean {
  if (!editor.mounted) return false;
  const { $from } = editor.state.selection;
  // Meowdown's inline-code mark uses mdCode rather than ProseMirror's spec.code flag.
  return !$from.parent.type.spec.code && !$from.marks().some((mark) => mark.type.name === "mdCode");
}

/** Uses the editor's autocomplete primitives, including keyboard navigation, IME handling and stale-range protection. */
export function NoteLinkAutocomplete({ onSearchNotes, onSearchEntities }: {
  onSearchNotes?(query: string): Promise<WikilinkItem[]>;
  onSearchEntities?(query: string): Promise<NoteLinkChoice[]>;
}) {
  const editor = useEditor<EditorExtension>();
  const allowed = useEditorDerivedValue<EditorExtension, boolean>(acceptsLinks);
  useExtension(useMemo(() => defineWikilinkTrigger(() => Boolean(onSearchNotes)), [onSearchNotes]));
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<SearchResult | null>(null);
  useEffect(() => {
    if (!allowed || !open || !query) return;
    let cancelled = false;
    const timer = window.setTimeout(() => {
      const notes = query.startsWith("[[");
      const term = query.slice(notes ? 2 : 1).trim();
      const search = async (): Promise<Choice[]> => notes
        ? (await onSearchNotes?.(term) ?? []).map((item) => ({ key: item.target, label: item.label ?? item.target, detail: item.detail, markdown: `[[${item.target}]]`, kind: "note" }))
        : (await onSearchEntities?.(term) ?? []).filter((item) => item.reference.kind === "contact" || item.reference.kind === "conversation").map((item) => ({ key: noteReferenceHref(item.reference), label: item.label, detail: noteLinkDetail(item), markdown: markdownEntityLink(item.label, item.reference), kind: item.reference.kind as "contact" | "conversation" }));
      void search().then((items) => { if (!cancelled) setResult({ query, items }); }, (reason) => { if (!cancelled) setResult({ query, items: [], error: errorMessage(reason) }); });
    }, 150);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [allowed, open, query, onSearchNotes, onSearchEntities]);
  const current = result?.query === query ? result : null;
  const items = current?.items ?? [];
  return <AutocompleteRoot regex={allowed ? NOTE_LINK_TRIGGER : null} queryBuilder={queryFromMatch} filter={showAll} followCursor
    onOpenChange={(event) => { setOpen(event.detail); if (!event.detail) setResult(null); }} onQueryChange={(event) => setQuery(event.detail)}>
    <AutocompletePositioner className="note-autocomplete-positioner">
      <AutocompletePopup className="note-autocomplete-popup" aria-label={query.startsWith("[[") ? "Notes" : "Contacts and conversations"}>
        {(["note", "contact", "conversation"] as const).map((kind) => {
          const group = items.filter((item) => item.kind === kind);
          if (!group.length) return null;
          return <div key={kind} role="group" aria-label={kind === "note" ? "Notes" : kind === "contact" ? "Contacts" : "Mail"}>
            {kind !== "note" ? <p className="note-autocomplete-heading" aria-hidden="true">{kind === "contact" ? "Contacts" : "Mail"}</p> : null}
            {group.map((item) => <AutocompleteItem key={item.key} value={item.key} className="note-autocomplete-item" onSelect={() => editor.commands.insertText({ text: item.markdown })}>
              <span className="note-autocomplete-icon" aria-hidden="true">{kind === "note" ? <NotebookText size={ICON_SIZE.sm} /> : kind === "contact" ? <UserRound size={ICON_SIZE.sm} /> : <Mail size={ICON_SIZE.sm} />}</span>
              <span className="note-autocomplete-text"><span>{item.label}</span>{item.detail ? <small>{item.detail}</small> : null}</span>
            </AutocompleteItem>)}
          </div>;
        })}
        <AutocompleteEmpty className="note-autocomplete-status" role="status">{current?.error ?? (!current ? "Searching…" : query.startsWith("[[") ? "No matching notes." : "No matching contacts or conversations.")}</AutocompleteEmpty>
      </AutocompletePopup>
    </AutocompletePositioner>
  </AutocompleteRoot>;
}
