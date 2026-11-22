import { Check, Pencil, Trash } from "lucide-react";
import { useMemo, useState } from "react";
import type { Label } from "./domain";
import { Modal } from "./AppChrome";
import { FindOrCreatePicker } from "./FindOrCreatePicker";
import { formatLabelName, isManageableLabel, type LabelCapabilities } from "./labels";
import { readLabelUsage, recordLabelUsed } from "./settings";
import { ICON_SIZE } from "./iconSizes";
import { InlineConfirm } from "./InlineConfirm";
import { errorMessage } from "./errors";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

export function LabelManager({
  labels,
  accountId,
  capabilities,
  nativeLabels = false,
  foldersOnly = false,
  checkedLabelIds,
  onClose,
  onCreate,
  onDelete,
  onRename,
  onToggle,
}: {
  labels: Label[];
  accountId?: string;
  /** What label and folder actions this account supports. */
  capabilities: LabelCapabilities;
  nativeLabels?: boolean;
  foldersOnly?: boolean;
  checkedLabelIds: Set<string>;
  onClose(): void;
  onCreate(name: string, folder?: boolean): Promise<Label>;
  onDelete(id: string): Promise<void>;
  onRename(id: string, name: string): Promise<void>;
  onToggle(label: Label, value: boolean): void;
}) {
  const [showFolders, setShowFolders] = useState(false);
  const folders = capabilities.canManageFolders && (foldersOnly || showFolders || !capabilities.canManageLabels);
  const [renaming, setRenaming] = useState<Label | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [deleting, setDeleting] = useState<Label | null>(null);
  const [error, setError] = useState<string | null>(null);

  const deleteLabel = async (label: Label) => {
    setBusy(true);
    setError(null);
    try {
      await onDelete(label.id);
      setDeleting(null);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  // Labels the user hasn't touched sort alphabetically; ones applied or
  // removed through Dispatch before bubble to the top, most-recent first,
  // so the label you're about to reach for is usually already near the top
  // before you've typed anything.
  const orderedLabels = useMemo(() => {
    const usage = accountId ? readLabelUsage(accountId) : {};
    return labels
      .filter((label) => folders ? label.kind === "folder" : isManageableLabel(label))
      .sort((a, b) => {
        const recencyDelta = (usage[b.id] ?? 0) - (usage[a.id] ?? 0);
        if (recencyDelta !== 0) return recencyDelta;
        return formatLabelName(a).localeCompare(formatLabelName(b), undefined, { sensitivity: "base" });
      });
  }, [labels, accountId, folders]);

  const startRename = (label: Label) => {
    setError(null);
    setRenaming(label);
    setRenameValue(label.name);
  };

  const selectLabel = (label: Label) => {
    if (busy || deleting) return;
    if (folders) {
      startRename(label);
      return;
    }
    onToggle(label, !checkedLabelIds.has(label.id));
    if (accountId) recordLabelUsed(accountId, label.id);
    onClose();
  };

  const createAndApply = async (name: string) => {
    const trimmed = name.trim();
    if (!trimmed || busy || deleting) return;
    setBusy(true);
    setError(null);
    try {
      if (folders) {
        await onCreate(trimmed, true);
      } else {
        const label = await onCreate(trimmed);
        onToggle(label, true);
        if (accountId) recordLabelUsed(accountId, label.id);
        onClose();
      }
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title={folders ? "Manage Folders" : "Manage Labels"} onClose={onClose}>
      {!foldersOnly && capabilities.canManageFolders && capabilities.canManageLabels ? (
        <button type="button" className="btn btn-sm" disabled={busy || Boolean(deleting)}
          onClick={() => {
            setShowFolders(!folders);
            setRenaming(null);
            setError(null);
          }}>
          {folders ? "Manage Labels" : "Manage Folders"}
        </button>
      ) : null}
      {!(capabilities.canManageLabels || folders) ? (
        <p className="label-unavailable">{capabilities.unavailableReason}</p>
      ) : (
      <FindOrCreatePicker
        items={orderedLabels}
        getSearchText={(label: Label) => formatLabelName(label)}
        key={folders ? "folders" : "labels"}
        placeholder={folders ? "Find or create a folder" : capabilities.canCreateLabels ? "Find or create a label" : "Find a label"}
        ariaLabel={folders ? "Find or Create a Folder" : capabilities.canCreateLabels ? "Find or Create a Label" : "Find a Label"}
        allowCreate={folders || capabilities.canCreateLabels}
        listId="label-options"
        listLabel={folders ? "Folders" : "Labels"}
        emptyMessage={folders ? "No matching folders." : capabilities.canCreateLabels ? "No labels yet. Type a name to create one." : "No matching labels."}
        createLabel={(name) => <>Create {folders ? "folder" : "label"} "{name}"</>}
        onSelect={selectLabel}
        onCreate={(name) => { void createAndApply(name); }}
        renderItem={(label, option) => (
          <div
            key={label.id}
            id={option.id}
            role="option"
            aria-label={!folders && checkedLabelIds.has(label.id) ? `${formatLabelName(label)}, added` : formatLabelName(label)}
            aria-selected={option.active}
            className={option.active ? "highlighted" : undefined}
            onMouseEnter={option.onMouseEnter}
            onClick={option.onClick}
          >
            <span className="label-option-name">
              {!folders && checkedLabelIds.has(label.id) ? <Check size={ICON_SIZE.sm} /> : <span className="label-option-check-spacer" />}
              {formatLabelName(label)}
            </span>
            {(folders || label.kind === "user") ? (
              <span className="label-actions">
                {folders || capabilities.canRenameLabels ? (
                  <button
                    className="btn-icon btn-icon-sm"
                    aria-label={`Rename ${label.name}`}
                    disabled={busy || Boolean(deleting)}
                    onClick={(event) => {
                      event.stopPropagation();
                      startRename(label);
                    }}
                  >
                    <Pencil size={ICON_SIZE.sm} />
                  </button>
                ) : null}
                {folders || capabilities.canDeleteLabels ? <button
                  className="btn-icon btn-icon-sm"
                  aria-label={`Delete ${label.name}`}
                  disabled={busy || Boolean(deleting)}
                  onClick={(event) => {
                    event.stopPropagation();
                    setRenaming(null);
                    setError(null);
                    if (nativeLabels || capabilities.canManageFolders) setDeleting(label);
                    else void deleteLabel(label);
                  }}
                >
                  <Trash size={ICON_SIZE.sm} />
                </button> : null}
              </span>
            ) : null}
          </div>
        )}
      />
      )}
      {renaming ? (
        <form
          className="rename-label"
          onSubmit={(event) => {
            event.preventDefault();
            if (!renameValue.trim() || busy) return;
            setBusy(true);
            setError(null);
            void onRename(renaming.id, renameValue)
              .then(() => setRenaming(null))
              .catch((reason) => setError(errorMessage(reason)))
              .finally(() => setBusy(false));
          }}
        >
          <input
            {...SEARCH_INPUT_ATTRIBUTES}
            autoFocus
            value={renameValue}
            onChange={(event) => setRenameValue(event.target.value)}
            aria-label={`Rename ${renaming.name}`}
          />
          <button type="button" className="btn btn-sm" onClick={() => setRenaming(null)}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={!renameValue.trim() || busy}>Save</button>
        </form>
      ) : null}
      {deleting ? <InlineConfirm
        ariaLabel={`Delete ${deleting.name} confirmation`}
        title={`Delete ${formatLabelName(deleting)}?`}
        stacked
        focusCancel
        cancelLabel="Cancel"
        onCancel={() => { setDeleting(null); setError(null); }}
        disabled={busy}
        actions={[{ label: folders ? "Delete Folder" : "Delete Label", className: "btn-danger", onClick: () => { void deleteLabel(deleting); } }]}
      >
        {nativeLabels
          ? "Your messages stay in Gmail. Nested labels stay in place. Split Inboxes that use this label will need a replacement."
          : `Each message must have another verified server copy. Split Inboxes that use this ${folders ? "folder" : "label"} will need a replacement.`}
      </InlineConfirm> : null}
      {error ? <p role="alert">{error}</p> : null}
    </Modal>
  );
}
