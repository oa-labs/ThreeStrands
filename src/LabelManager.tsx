import { Check, Pencil, Trash } from "lucide-react";
import { useMemo, useState } from "react";
import type { Label } from "./domain";
import { Modal } from "./AppChrome";
import { FindOrCreatePicker } from "./FindOrCreatePicker";
import { formatLabelName, isManageableLabel } from "./labels";
import { readLabelUsage, recordLabelUsed } from "./settings";
import { ICON_SIZE } from "./iconSizes";

export function LabelManager({
  labels,
  accountId,
  checkedLabelIds,
  onClose,
  onCreate,
  onDelete,
  onRename,
  onToggle,
}: {
  labels: Label[];
  accountId?: string;
  checkedLabelIds: Set<string>;
  onClose(): void;
  onCreate(name: string): Promise<Label>;
  onDelete(id: string): Promise<void>;
  onRename(id: string, name: string): Promise<void>;
  onToggle(label: Label, value: boolean): void;
}) {
  const [renaming, setRenaming] = useState<Label | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [busy, setBusy] = useState(false);

  // Labels the user hasn't touched sort alphabetically; ones applied or
  // removed through Dispatch before bubble to the top, most-recent first,
  // so the label you're about to reach for is usually already near the top
  // before you've typed anything.
  const orderedLabels = useMemo(() => {
    const usage = accountId ? readLabelUsage(accountId) : {};
    return labels
      .filter(isManageableLabel)
      .sort((a, b) => {
        const recencyDelta = (usage[b.id] ?? 0) - (usage[a.id] ?? 0);
        if (recencyDelta !== 0) return recencyDelta;
        return formatLabelName(a).localeCompare(formatLabelName(b), undefined, { sensitivity: "base" });
      });
  }, [labels, accountId]);

  const applyLabel = (label: Label) => {
    onToggle(label, !checkedLabelIds.has(label.id));
    if (accountId) recordLabelUsed(accountId, label.id);
    onClose();
  };

  const createAndApply = async (name: string) => {
    const trimmed = name.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    try {
      const label = await onCreate(trimmed);
      onToggle(label, true);
      if (accountId) recordLabelUsed(accountId, label.id);
      onClose();
    } catch {
      setBusy(false);
    }
  };

  return (
    <Modal title="Manage Labels" onClose={onClose}>
      <FindOrCreatePicker
        items={orderedLabels}
        getSearchText={(label: Label) => formatLabelName(label)}
        placeholder="Find or create a label"
        ariaLabel="Find or Create a Label"
        listId="label-options"
        listLabel="Labels"
        emptyMessage="No labels yet. Type a name to create one."
        createLabel={(name) => <>Create label "{name}"</>}
        onSelect={applyLabel}
        onCreate={(name) => { void createAndApply(name); }}
        renderItem={(label, option) => (
          <div
            key={label.id}
            id={option.id}
            role="option"
            aria-label={checkedLabelIds.has(label.id) ? `${formatLabelName(label)}, added` : formatLabelName(label)}
            aria-selected={option.active}
            className={option.active ? "highlighted" : undefined}
            onMouseEnter={option.onMouseEnter}
            onClick={option.onClick}
          >
            <span className="label-option-name">
              {checkedLabelIds.has(label.id) ? <Check size={ICON_SIZE.sm} /> : <span className="label-option-check-spacer" />}
              {formatLabelName(label)}
            </span>
            {label.kind === "user" ? (
              <span className="label-actions">
                <button
                  className="btn-icon btn-icon-sm"
                  aria-label={`Rename ${label.name}`}
                  onClick={(event) => {
                    event.stopPropagation();
                    setRenaming(label);
                    setRenameValue(label.name);
                  }}
                >
                  <Pencil size={ICON_SIZE.sm} />
                </button>
                <button
                  className="btn-icon btn-icon-sm"
                  aria-label={`Delete ${label.name}`}
                  onClick={(event) => {
                    event.stopPropagation();
                    void onDelete(label.id);
                  }}
                >
                  <Trash size={ICON_SIZE.sm} />
                </button>
              </span>
            ) : null}
          </div>
        )}
      />
      {renaming ? (
        <form
          className="rename-label"
          onSubmit={(event) => {
            event.preventDefault();
            if (!renameValue.trim() || busy) return;
            setBusy(true);
            void onRename(renaming.id, renameValue)
              .then(() => setRenaming(null))
              .finally(() => setBusy(false));
          }}
        >
          <input
            autoFocus
            value={renameValue}
            onChange={(event) => setRenameValue(event.target.value)}
            aria-label={`Rename ${renaming.name}`}
          />
          <button type="button" className="btn btn-sm" onClick={() => setRenaming(null)}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={!renameValue.trim() || busy}>Save</button>
        </form>
      ) : null}
    </Modal>
  );
}
