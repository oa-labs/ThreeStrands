import "@meowdown/core/style.css";
import "@meowdown/react/style.css";
import { useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type Ref } from "react";
import { MarkdownEditor, useExtension, useKeymap, type EditorHandle, type WikilinkItem } from "@meowdown/react";
import { Priority, defineViewAttributes } from "@meowdown/core";
import { openNoteLink } from "./noteLinks";
import { splitWikiLink } from "./noteWikiLinks";
import type { NoteSyntaxMode } from "./noteSyntaxMode";

export type NoteEditorHandle = {
  focus(): void;
  /** The Markdown the editor holds now, as it would be saved. */
  markdown(): string;
};

/** The editor library's name for each Markdown Syntax choice. */
const MARK_MODE = { hide: "hide", hybrid: "focus", show: "show" } as const;

/**
 * Remote content stays off: no image, X post, or YouTube card resolves, so
 * opening a note never reaches the network. The app's CSP blocks remote
 * images and connections as well.
 */
const noRemoteContent = () => undefined;

/** Shows `[[Title|shown text]]` as its shown text, and follows it to Title. */
const resolveWikilink = ({ target }: { target: string }) => {
  const { target: title, label } = splitWikiLink(target);
  return title ? { target: title, display: label } : undefined;
};

/** A note the `[[` picker offers. */
export type NoteLinkSuggestion = { title: string; detail?: string };

/**
 * The `[[` picker's rows: the matching notes, then what was typed when no
 * note has that title, which links to a note that doesn't exist yet;
 * following that link creates it.
 */
export function linkMenuItems(query: string, found: NoteLinkSuggestion[]): WikilinkItem[] {
  const typed = query.split(/\s+/).filter(Boolean).join(" ");
  const exact = found.some((note) => note.title.toLocaleLowerCase() === typed.toLocaleLowerCase());
  return [
    ...found.map((note): WikilinkItem => ({ target: note.title, detail: note.detail })),
    ...(typed && !exact && !/[[\]|]/.test(typed) ? [{ target: typed, detail: "New note" }] : []),
  ];
}

/** Names the editable root, which the editor library creates itself. */
function EditableName({ label }: { label: string }) {
  useExtension(useMemo(() => defineViewAttributes({ role: "textbox", "aria-multiline": "true", "aria-label": label }), [label]));
  return null;
}

/**
 * Escape hands focus back to the note list. It runs after the editor's
 * menus, which close on Escape first, and before ProseMirror's own Escape
 * (select the enclosing block), which would otherwise take every press. A
 * selection collapses first, as in the rest of the editor.
 */
function EscapeLeaves({ onLeave }: { onLeave(): void }) {
  const latest = useRef(onLeave);
  useEffect(() => { latest.current = onLeave; });
  const keymap = useMemo(() => ({
    Escape: (state: { selection: { empty: boolean } }) => {
      if (!state.selection.empty) return false;
      latest.current();
      return true;
    },
  }), []);
  useKeymap(keymap, { priority: Priority.default });
  return null;
}

/**
 * The note's writing surface: a Markdown editor that shows formatting in
 * place. The stored value is always the Markdown text. The editor reports
 * text only after the user changes something, so opening a note never
 * rewrites it. Escape hands focus back to the note list once the editor has
 * nothing of its own to close.
 */
export function NoteEditor({ ref, noteKey, value, label, placeholder, readOnly = false, mode, onChange, onLeave, onSearchLinks, onOpenLink }: {
  ref?: Ref<NoteEditorHandle>;
  /** Changes when another note opens, which starts a fresh editor. */
  noteKey: string;
  value: string;
  label: string;
  placeholder?: string;
  readOnly?: boolean;
  mode: NoteSyntaxMode;
  onChange(value: string): void;
  onLeave(): void;
  /** Notes the `[[` picker offers for what has been typed. */
  onSearchLinks?(query: string): Promise<NoteLinkSuggestion[]>;
  /** Opens the note a `[[link]]` names. */
  onOpenLink?(title: string): void;
}) {
  const editor = useRef<EditorHandle>(null);
  const surface = useRef<HTMLDivElement>(null);
  // The Markdown the editor holds, as last reported or loaded. A different
  // `value` from outside (the note loading, a sync, a chosen version) starts
  // a fresh editor, so undo can't step back past it and save the old text;
  // the editor's own reports coming back as `value` don't.
  const shown = useRef(value);
  const [generation, setGeneration] = useState(0);
  const refocus = useRef(false);
  const latestChange = useRef(onChange);
  const latestLinks = useRef({ onSearchLinks, onOpenLink });
  useEffect(() => { latestChange.current = onChange; latestLinks.current = { onSearchLinks, onOpenLink }; });

  useImperativeHandle(ref, () => ({ focus: () => editor.current?.focus(), markdown: () => editor.current?.getMarkdown() ?? shown.current }), []);

  useEffect(() => {
    if (value === shown.current) return;
    shown.current = value;
    refocus.current = Boolean(surface.current?.contains(document.activeElement));
    setGeneration((current) => current + 1);
  }, [value]);
  useEffect(() => {
    if (!refocus.current) return;
    refocus.current = false;
    editor.current?.focus();
  }, [generation]);

  const handleDocChange = useCallback(() => {
    const markdown = editor.current?.getMarkdown();
    if (markdown === undefined || markdown === shown.current) return;
    shown.current = markdown;
    latestChange.current(markdown);
  }, []);
  const handleLink = useCallback(({ href }: { href: string }) => openNoteLink(href), []);
  const handleWikilink = useCallback(({ target }: { target: string }) => latestLinks.current.onOpenLink?.(target), []);
  const searchLinks = useCallback(async (query: string) => linkMenuItems(query, await latestLinks.current.onSearchLinks?.(query) ?? []), []);
  const reducedMotion = typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  return <div ref={surface} className="note-editor-body" onClickCapture={(event) => {
    // A link in a note never navigates the app's own window, even read-only;
    // the editor reports the click and `openNoteLink` decides what opens.
    if (event.target instanceof Element && event.target.closest("a[href]")) event.preventDefault();
  }}>
    <MarkdownEditor
      key={`${noteKey}:${generation}`}
      handleRef={editor}
      mode={MARK_MODE[mode]}
      initialMarkdown={value}
      onDocChange={handleDocChange}
      onLinkClick={handleLink}
      onWikilinkClick={handleWikilink}
      onWikilinkSearch={onSearchLinks ? searchLinks : undefined}
      resolveWikilink={resolveWikilink}
      readOnly={readOnly}
      placeholder={placeholder}
      spellCheck
      resolveImageUrl={noRemoteContent}
      resolveXPost={noRemoteContent}
      resolveYouTubeVideo={noRemoteContent}
      embedPaste={false}
      frontmatter
      caretGlide={!reducedMotion}
      wrapperClassName="note-editor-surface"
    >
      <EditableName label={label} />
      <EscapeLeaves onLeave={onLeave} />
    </MarkdownEditor>
  </div>;
}
