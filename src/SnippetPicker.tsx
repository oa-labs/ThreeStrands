import { useEffect, useMemo, useState } from "react";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { Modal } from "./AppChrome";
import type { Snippet } from "./domain";
import { readSnippetUsage } from "./settings";
import { linkifyPlainText, sanitizeComposeHtml } from "./richText";
import { snippetBodyPreview } from "./snippets";

function htmlToPlainText(html: string): string {
  const container = document.createElement("div");
  container.innerHTML = html.replace(/<br\s*\/?>/gi, "\n").replace(/<\/(p|div)>/gi, "\n");
  return (container.textContent ?? "").replace(/\n{3,}/g, "\n\n").trim();
}

export function SnippetPicker({
  snippets,
  onClose,
  onInsert,
  onCreate,
  onUpdate,
  onDelete,
}: {
  snippets: Snippet[];
  onClose(): void;
  onInsert(snippet: Snippet): void;
  onCreate(name: string, body: string): Promise<Snippet>;
  onUpdate(id: string, name: string, body: string): Promise<Snippet>;
  onDelete(id: string): Promise<void>;
}) {
  const [query, setQuery] = useState("");
  const [highlightedIndex, setHighlightedIndex] = useState(0);
  const [editorTarget, setEditorTarget] = useState<Snippet | "new" | null>(null);

  const orderedSnippets = useMemo(() => {
    const usage = readSnippetUsage();
    return [...snippets].sort((a, b) => {
      const recencyDelta = (usage[b.id] ?? 0) - (usage[a.id] ?? 0);
      if (recencyDelta !== 0) return recencyDelta;
      return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
    });
  }, [snippets]);

  const normalizedQuery = query.trim().toLowerCase();
  const filteredSnippets = normalizedQuery
    ? orderedSnippets.filter((snippet) => snippet.name.toLowerCase().includes(normalizedQuery))
    : orderedSnippets;
  const exactMatchExists = orderedSnippets.some((snippet) => snippet.name.toLowerCase() === normalizedQuery);
  const showCreateRow = normalizedQuery.length > 0 && !exactMatchExists;
  const rowCount = filteredSnippets.length + (showCreateRow ? 1 : 0);
  const activeIndex = rowCount === 0 ? -1 : Math.min(Math.max(highlightedIndex, 0), rowCount - 1);

  useEffect(() => {
    setHighlightedIndex(0);
  }, [query]);

  const selectRow = (index: number) => {
    if (index < filteredSnippets.length) {
      onInsert(filteredSnippets[index]);
    } else if (showCreateRow) {
      setEditorTarget("new");
    }
  };

  if (editorTarget) {
    return (
      <SnippetEditor
        target={editorTarget}
        initialName={editorTarget === "new" ? query.trim() : editorTarget.name}
        onClose={onClose}
        onBack={() => setEditorTarget(null)}
        onCreate={async (name, body) => {
          const snippet = await onCreate(name, body);
          onInsert(snippet);
        }}
        onUpdate={async (id, name, body) => {
          await onUpdate(id, name, body);
          setEditorTarget(null);
        }}
      />
    );
  }

  return (
    <Modal title="Insert snippet" onClose={onClose} className="snippet-picker">
      <div className="label-search">
        <input
          autoFocus
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Find or create a snippet"
          aria-label="Find or create a snippet"
          role="combobox"
          aria-expanded="true"
          aria-controls="snippet-options"
          aria-activedescendant={activeIndex >= 0 ? `snippet-option-${activeIndex}` : undefined}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown") {
              event.preventDefault();
              setHighlightedIndex((index) => Math.min(index + 1, Math.max(rowCount - 1, 0)));
            } else if (event.key === "ArrowUp") {
              event.preventDefault();
              setHighlightedIndex((index) => Math.max(index - 1, 0));
            } else if (event.key === "Enter") {
              event.preventDefault();
              if (activeIndex >= 0) selectRow(activeIndex);
            }
          }}
        />
      </div>
      <div className="label-list" id="snippet-options" role="listbox" aria-label="Snippets">
        {filteredSnippets.map((snippet, index) => (
          <div
            key={snippet.id}
            id={`snippet-option-${index}`}
            role="option"
            aria-selected={index === activeIndex}
            className={index === activeIndex ? "highlighted" : undefined}
            onMouseEnter={() => setHighlightedIndex(index)}
            onClick={() => selectRow(index)}
          >
            <span className="snippet-option-name">
              <strong>{snippet.name}</strong>
              <span className="snippet-option-preview">{snippetBodyPreview(snippet.body)}</span>
            </span>
            <span className="label-actions">
              <button
                aria-label={`Edit ${snippet.name}`}
                onClick={(event) => {
                  event.stopPropagation();
                  setEditorTarget(snippet);
                }}
              >
                <Pencil size={14} />
              </button>
              <button
                aria-label={`Delete ${snippet.name}`}
                onClick={(event) => {
                  event.stopPropagation();
                  void onDelete(snippet.id);
                }}
              >
                <Trash2 size={14} />
              </button>
            </span>
          </div>
        ))}
        {showCreateRow ? (
          <div
            id={`snippet-option-${filteredSnippets.length}`}
            role="option"
            aria-selected={filteredSnippets.length === activeIndex}
            className={filteredSnippets.length === activeIndex ? "highlighted" : undefined}
            onMouseEnter={() => setHighlightedIndex(filteredSnippets.length)}
            onClick={() => setEditorTarget("new")}
          >
            <Plus size={14} /> Create snippet "{query.trim()}"
          </div>
        ) : null}
        {filteredSnippets.length === 0 && !showCreateRow ? (
          <p className="empty">No snippets yet. Type a name to create one.</p>
        ) : null}
      </div>
    </Modal>
  );
}

export function SnippetEditor({
  target,
  initialName,
  backLabel = "Back",
  onClose,
  onBack,
  onCreate,
  onUpdate,
}: {
  target: Snippet | "new";
  initialName: string;
  backLabel?: string;
  onClose(): void;
  onBack(): void;
  onCreate(name: string, body: string): Promise<void>;
  onUpdate(id: string, name: string, body: string): Promise<void>;
}) {
  const [name, setName] = useState(initialName);
  const [body, setBody] = useState(target === "new" ? "" : htmlToPlainText(target.body));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async () => {
    const trimmedName = name.trim();
    const trimmedBody = body.trim();
    if (!trimmedName || !trimmedBody || busy) return;
    setBusy(true);
    setError(null);
    try {
      const html = sanitizeComposeHtml(linkifyPlainText(trimmedBody));
      if (target === "new") await onCreate(trimmedName, html);
      else await onUpdate(target.id, trimmedName, html);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Could not save the snippet");
      setBusy(false);
    }
  };

  return (
    <Modal title={target === "new" ? "New snippet" : "Edit snippet"} onClose={onClose} className="snippet-editor">
      <form
        className="snippet-editor-form"
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <label className="snippet-field">
          <span>Name</span>
          <input autoFocus value={name} onChange={(event) => setName(event.target.value)} disabled={busy} />
        </label>
        <label className="snippet-field">
          <span>Body</span>
          <textarea
            rows={6}
            value={body}
            onChange={(event) => setBody(event.target.value)}
            disabled={busy}
            placeholder={"Use {first_name} to insert the recipient's first name"}
          />
        </label>
        {error ? <p className="form-error" role="alert">{error}</p> : null}
        <div className="snippet-editor-actions">
          <button type="button" onClick={onBack} disabled={busy}>{backLabel}</button>
          <button type="submit" disabled={!name.trim() || !body.trim() || busy}>Save</button>
        </div>
      </form>
    </Modal>
  );
}
