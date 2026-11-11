import { notePreview } from "./notePreview";
import { fromEditorMarkdown, toEditorMarkdown } from "./noteEntities";
import "@meowdown/core/style.css";
import "@meowdown/react/style.css";
import { useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type Ref } from "react";
import { MarkdownEditor, useExtension, useKeymap, type EditorHandle, type WikilinkItem } from "@meowdown/react";
import { Priority, defineViewAttributes } from "@meowdown/core";
import { attachmentIdOf, type NoteFiles } from "./noteAttachments";
import { openNoteLink } from "./noteLinks";
import { collapseWhitespace, noteTitleKey, splitWikiLink } from "./noteWikiLinks";
import type { NoteSyntaxMode } from "./noteSyntaxMode";
import { NoteLinkAutocomplete } from "./NoteLinkAutocomplete";
import type { NoteLinkChoice } from "./noteLinkChoices";
import { formattingShortcutFor } from "./richText";

export type NoteEditorHandle = {
  focus(): void;
  /** The Markdown the editor holds now, as it would be saved. */
  markdown(): string;
  insert(markdown: string): void;
  revealTask(markerId: string): void;
  revealReference(href: string): void;
};

/** The editor library's name for each Markdown Syntax choice. */
const MARK_MODE = { hide: "hide", hybrid: "focus", show: "show" } as const;

/**
 * Remote content stays off: no remote image, X post, or YouTube card
 * resolves, so opening a note never reaches the network. The app's CSP
 * blocks remote images and connections as well. The only images shown are
 * files attached to notes, which come from this device as `data:` URLs.
 */
const noRemoteContent = () => undefined;

/** `[name](attachment:<id>)` shows as a file pill rather than a link. */
const claimAttachmentLink = ({ href }: { href: string }) => attachmentIdOf(href) !== null;

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
  const typed = collapseWhitespace(query);
  const exact = found.some((note) => noteTitleKey(note.title) === noteTitleKey(typed));
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
export function NoteEditor({ ref, noteKey, value, label, placeholder, readOnly = false, mode, files, onChange, onLeave, onSearchLinks, onSearchEntities, onOpenLink, onOpenEntity }: {
  ref?: Ref<NoteEditorHandle>;
  /** Changes when another note opens, which starts a fresh editor. */
  noteKey: string;
  value: string;
  label: string;
  placeholder?: string;
  readOnly?: boolean;
  mode: NoteSyntaxMode;
  /** Stores pasted and dropped files and shows attached ones. Without it, files can't be added. */
  files?: NoteFiles;
  onChange(value: string): void;
  onLeave(): void;
  /** Notes the `[[` picker offers for what has been typed. */
  onSearchLinks?(query: string): Promise<NoteLinkSuggestion[]>;
  /** Contacts and conversations the @ picker offers. */
  onSearchEntities?(query: string): Promise<NoteLinkChoice[]>;
  /** Opens the note a `[[link]]` names. */
  onOpenLink?(title: string): void;
  onOpenEntity?(href: string): void;
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
  const latestLinks = useRef({ onSearchLinks, onSearchEntities, onOpenLink, onOpenEntity });
  const latestFiles = useRef(files);
  useEffect(() => { latestChange.current = onChange; latestLinks.current = { onSearchLinks, onSearchEntities, onOpenLink, onOpenEntity }; latestFiles.current = files; });

  useImperativeHandle(ref, () => ({ focus: () => editor.current?.focus(), markdown: () => fromEditorMarkdown(editor.current?.getMarkdown() ?? toEditorMarkdown(shown.current), shown.current), insert: (markdown) => editor.current?.insertMarkdown(toEditorMarkdown(markdown)), revealTask: (id) => {
    const e = editor.current?.getEditor();
    if (!e) return;
    let at: number | null = null;
    let paragraph = 1;
    e.state.doc.descendants((node, pos) => {
      if (node.isTextblock) paragraph = pos + 1;
      if (node.textContent.includes(`threestrands-task:${id}`) && node.isTextblock) at ??= pos + 1;
      if (typeof node.attrs.content === "string" && node.attrs.content.includes(`threestrands-task:${id}`)) at ??= paragraph;
    });
    if (at !== null) { editor.current?.setSelection({ type: "text", anchor: at, head: at }); editor.current?.focus(); editor.current?.scrollIntoView(); }
  }, revealReference: (href) => {
    const e = editor.current?.getEditor();
    if (!e) return;
    let at: number | null = null;
    e.state.doc.descendants((node, pos) => {
      if (node.isTextblock && node.textContent.includes(href)) at ??= pos + Math.max(1, node.textContent.indexOf(href));
      if (node.attrs.href === href) at ??= pos;
    });
    if (at !== null) { editor.current?.setSelection({ type: "text", anchor: at, head: at }); editor.current?.focus(); editor.current?.scrollIntoView(); }
  } }), []);

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

  useEffect(() => {
    let dispose: (() => void) | undefined;
    const timer = window.setTimeout(() => {
      const view = editor.current?.getEditor()?.view;
      const base = view?.someProp("nodeViews", (nodes) => nodes.list);
      if (!view || !base) return;
      const original = view.props.nodeViews;
      // Label controls inside the node view that owns them. Mutating rendered
      // editor DOM from an observer would race ProseMirror's reconciliation.
      view.setProps({ nodeViews: { ...original, list: (node, ...args) => {
        const rendered = base(node, ...args);
        const decorate = (current: typeof node) => {
          if (!(rendered.dom instanceof HTMLElement)) return;
          const checkbox = rendered.dom.querySelector<HTMLInputElement>(':scope > .list-marker input[type="checkbox"]');
          if (!checkbox || current.attrs.kind !== "task") return;
          checkbox.setAttribute("aria-label", `${current.attrs.marker === "+" ? "Task" : "Checklist"}: ${notePreview(current.firstChild?.textContent ?? "")}`);
          checkbox.tabIndex = readOnly ? -1 : 0;
          checkbox.disabled = readOnly;
        };
        decorate(node);
        return { ...rendered, update: (next, ...rest) => { if (!rendered.update?.(next, ...rest)) return false; decorate(next); return true; } };
      } } });
      dispose = () => { if (!view.isDestroyed) view.setProps({ nodeViews: original }); };

    }, 0);
    return () => { window.clearTimeout(timer); dispose?.(); };
  }, [generation, noteKey, readOnly]);

  const handleDocChange = useCallback(() => {
    const raw = editor.current?.getMarkdown();
    const markdown = raw === undefined ? undefined : fromEditorMarkdown(raw, shown.current);
    if (markdown === undefined || markdown === shown.current) return;
    shown.current = markdown;
    latestChange.current(markdown);
  }, []);
  const handleLink = useCallback(({ href }: { href: string }) => {
    if (href.startsWith("threestrands:")) latestLinks.current.onOpenEntity?.(href);
    else openNoteLink(href);
  }, []);
  const handleWikilink = useCallback(({ target }: { target: string }) => latestLinks.current.onOpenLink?.(target), []);
  const searchLinks = useCallback(async (query: string) => linkMenuItems(query, await latestLinks.current.onSearchLinks?.(query) ?? []), []);
  const searchEntities = useCallback(async (query: string) => await latestLinks.current.onSearchEntities?.(query) ?? [], []);
  // The editor library wants each of these to stay the same function.
  const saveFile = useCallback((file: File) => latestFiles.current?.save(file), []);
  const resolveImage = useCallback((src: string) => latestFiles.current?.image(src), []);
  const resolveFileInfo = useCallback((href: string) => latestFiles.current?.info(href), []);
  const openFile = useCallback(({ href }: { href: string }) => latestFiles.current?.open(href), []);
  const openImage = useCallback(({ src }: { src: string }) => { if (attachmentIdOf(src)) latestFiles.current?.open(src); }, []);
  const reducedMotion = typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  return <div ref={surface} className="note-editor-body" onKeyDownCapture={(event) => {
    // Use Mail's bindings where Meowdown differs. Keep edits in the editor's
    // commands so Markdown serialization, selection and undo stay together.
    const view = editor.current?.getEditor();
    if (!readOnly && view?.view.dom.contains(event.target as Node)
      && event.target instanceof Element && !event.target.closest("input, button, textarea, select")) {
      const shortcut = formattingShortcutFor(event.nativeEvent);
      const apply = shortcut?.id === "format.fixedWidth" ? () => view.commands.toggleCode()
        : shortcut?.id === "format.quote" ? () => view.commands.toggleBlockquote()
        : shortcut?.id === "format.codeBlock" ? () => view.commands.toggleCodeBlock()
        : undefined;
      if (apply) {
        event.stopPropagation();
        // Don't fall through to the library's checklist binding during IME.
        if (event.nativeEvent.isComposing) return;
        event.preventDefault();
        apply();
        return;
      }
    }
    if (readOnly || !(event.target instanceof HTMLInputElement) || event.target.type !== "checkbox" || ![" ", "Enter"].includes(event.key)) return;
    event.preventDefault(); event.stopPropagation();
    event.target.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
  }} onAuxClickCapture={(event) => {
    if (event.target instanceof Element && event.target.closest("a[href]")) event.preventDefault();
  }} onClickCapture={(event) => {
    // A link in a note never navigates the app's own window, even read-only;
    // the editor reports the click and `openNoteLink` decides what opens.
    if (event.target instanceof Element && event.target.closest("a[href]")) event.preventDefault();
  }}>
    <MarkdownEditor
      key={`${noteKey}:${generation}`}
      handleRef={editor}
      mode={MARK_MODE[mode]}
      initialMarkdown={toEditorMarkdown(value)}
      onDocChange={handleDocChange}
      onSlashMenuSearch={(query) => [
        { label: "Checklist", keywords: ["square"], onSelect: () => editor.current?.getEditor()?.commands.wrapInSquareTask() },
        { label: "Task", keywords: ["round", "todo"], onSelect: () => editor.current?.getEditor()?.commands.wrapInCircleTask() },
      ].filter((item) => item.label.toLowerCase().includes(query))}
      onLinkClick={handleLink}
      onWikilinkClick={handleWikilink}
      resolveWikilink={resolveWikilink}
      readOnly={readOnly}
      placeholder={placeholder}
      spellCheck
      resolveImageUrl={resolveImage}
      resolveFileLink={claimAttachmentLink}
      resolveFileInfo={resolveFileInfo}
      onFileClick={openFile}
      onImageClick={openImage}
      onFilePaste={files && !readOnly ? saveFile : undefined}
      resolveXPost={noRemoteContent}
      resolveYouTubeVideo={noRemoteContent}
      embedPaste={false}
      frontmatter
      caretGlide={!reducedMotion}
      wrapperClassName="note-editor-surface"
    >
      <EditableName label={label} />
      {!readOnly && (onSearchLinks || onSearchEntities) ? <NoteLinkAutocomplete onSearchNotes={onSearchLinks ? searchLinks : undefined} onSearchEntities={onSearchEntities ? searchEntities : undefined} /> : null}
      <EscapeLeaves onLeave={onLeave} />
    </MarkdownEditor>
  </div>;
}
