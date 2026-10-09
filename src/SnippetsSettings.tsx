import { Plus } from "lucide-react";
import { useState } from "react";
import type { Snippet } from "./domain";
import { snippetBodyPreview } from "./snippets";
import { SnippetEditor } from "./SnippetPicker";
import { useSettingsOperation } from "./settingsOperations";
import { ICON_SIZE } from "./iconSizes";

export function SnippetsSettings({
  snippets,
  onCreate,
  onUpdate,
  onDelete,
}: {
  snippets: Snippet[];
  onCreate(name: string, body: string): Promise<Snippet>;
  onUpdate(id: string, name: string, body: string): Promise<Snippet>;
  onDelete(id: string): Promise<void>;
}) {
  const [editorTarget, setEditorTarget] = useState<Snippet | "new" | null>(null);
  const { pending: busyId, error, runFor } = useSettingsOperation();

  const orderedSnippets = [...snippets].sort((a, b) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
  );

  return (
    <section className="settings-section accounts-manager" aria-label="Snippets">
      <div className="accounts-manager-header">
        <div>
          <h3>Snippets</h3>
          <p>
            Canned text you can insert into a reply with <kbd>⌘/Ctrl ;</kbd>. Use{" "}
            <code>{"{first_name}"}</code> to insert the recipient's first name.
          </p>
        </div>
        <button type="button" className="btn btn-primary" onClick={() => setEditorTarget("new")}>
          <Plus size={ICON_SIZE.md} /> Add Snippet
        </button>
      </div>
      {orderedSnippets.length === 0 ? (
        <div className="accounts-empty">
          <strong>No snippets yet</strong>
          <p>Add one above, or create one from the snippet picker (<kbd>⌘/Ctrl ;</kbd>) while composing.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {orderedSnippets.map((snippet) => (
            <li className="account-card" key={snippet.id}>
              <div className="account-card-row">
                <div className="account-card-identity">
                  <strong>{snippet.name}</strong>
                  <span className="account-card-email">{snippetBodyPreview(snippet.body)}</span>
                </div>
              </div>
              <div className="account-card-controls">
                <span className="accounts-list-actions">
                  <button type="button" className="btn btn-sm" onClick={() => setEditorTarget(snippet)}>
                    Edit
                  </button>
                  <button
                    type="button"
                    className="btn btn-sm btn-danger"
                    disabled={busyId !== null}
                    onClick={() => runFor(snippet.id, () => onDelete(snippet.id))}
                  >
                    Delete
                  </button>
                </span>
              </div>
            </li>
          ))}
        </ul>
      )}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      {editorTarget ? (
        <SnippetEditor
          target={editorTarget}
          initialName={editorTarget === "new" ? "" : editorTarget.name}
          backLabel="Cancel"
          onClose={() => setEditorTarget(null)}
          onBack={() => setEditorTarget(null)}
          onCreate={async (name, body) => { await onCreate(name, body); setEditorTarget(null); }}
          onUpdate={async (id, name, body) => { await onUpdate(id, name, body); setEditorTarget(null); }}
        />
      ) : null}
    </section>
  );
}
