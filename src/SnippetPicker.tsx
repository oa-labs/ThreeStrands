import { useMemo, useState } from "react";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { Modal } from "./AppChrome";
import type { Snippet } from "./domain";
import { readSnippetUsage } from "./settings";
import { linkifyPlainText, sanitizeComposeHtml } from "./richText";
import { snippetBodyPreview } from "./snippets";
import { FindOrCreatePicker } from "./FindOrCreatePicker";
import { htmlToPlainText } from "./htmlPlainText";

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
  const [editorTarget, setEditorTarget] = useState<Snippet | "new" | null>(null);
  const [newSnippetName, setNewSnippetName] = useState("");

  const orderedSnippets = useMemo(() => {
    const usage = readSnippetUsage();
    return [...snippets].sort((a, b) => {
      const recencyDelta = (usage[b.id] ?? 0) - (usage[a.id] ?? 0);
      if (recencyDelta !== 0) return recencyDelta;
      return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
    });
  }, [snippets]);

  if (editorTarget) {
    return (
      <SnippetEditor
        target={editorTarget}
        initialName={editorTarget === "new" ? newSnippetName : editorTarget.name}
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
    <Modal title="Insert Snippet" onClose={onClose} className="snippet-picker">
      <FindOrCreatePicker
        items={orderedSnippets}
        getSearchText={(snippet) => snippet.name}
        placeholder="Find or create a snippet"
        ariaLabel="Find or Create a Snippet"
        listId="snippet-options"
        listLabel="Snippets"
        emptyMessage="No snippets yet. Type a name to create one."
        createLabel={(name) => <><Plus size={14} /> Create snippet "{name}"</>}
        onSelect={onInsert}
        onCreate={(name) => { setNewSnippetName(name); setEditorTarget("new"); }}
        renderItem={(snippet, option) => (
          <div
            key={snippet.id}
            id={option.id}
            role="option"
            aria-selected={option.active}
            className={option.active ? "highlighted" : undefined}
            onMouseEnter={option.onMouseEnter}
            onClick={option.onClick}
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
        )}
        renderCreateItem={(name, option) => (
          <div id={option.id} role="option" aria-selected={option.active} className={option.active ? "highlighted" : undefined}
            onMouseEnter={option.onMouseEnter} onClick={option.onClick}>
            <Plus size={14} /> Create snippet "{name}"
          </div>
        )}
      />
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
  const [body, setBody] = useState(target === "new" ? "" : htmlToPlainText(target.body, { whitespace: "preserve" }));
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
    <Modal title={target === "new" ? "New Snippet" : "Edit Snippet"} onClose={onClose} className="snippet-editor">
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
          <button type="button" className="btn" onClick={onBack} disabled={busy}>{backLabel}</button>
          <button type="submit" className="btn btn-primary" disabled={!name.trim() || !body.trim() || busy}>Save</button>
        </div>
      </form>
    </Modal>
  );
}
