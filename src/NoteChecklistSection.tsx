import { useId, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { ICON_SIZE } from "./iconSizes";
import { ContextRow, ContextSection } from "./ContextSections";
import { MoreActionsMenu } from "./MoreActionsMenu";
import type { NoteItem } from "./noteEntities";

/** The saved note remains the source of order and state; grouping only changes its preview. */
export function NoteChecklistSection({ items, disabled, onChange }: {
  items: NoteItem[];
  disabled: boolean;
  onChange(item: NoteItem, patch: { checked?: boolean; task?: boolean }): void;
}) {
  const [showCompleted, setShowCompleted] = useState(false);
  const completedId = useId();
  const pending = items.filter((item) => !item.checked);
  const completed = items.filter((item) => item.checked);
  const row = (item: NoteItem) => <NoteItemRow key={item.id} item={item} disabled={disabled} onChange={onChange} />;
  return <ContextSection id="note-checklist-tasks" title="Checklists and tasks" count={items.length}
    className="note-checklist-section" rows={pending.map(row)} closing={completed.length ? <div className="note-completed">
      <button type="button" className="btn-link note-completed-toggle" aria-expanded={showCompleted} aria-controls={completedId} onClick={() => setShowCompleted(!showCompleted)}>
        <span className="context-row-glyph" aria-hidden="true">{showCompleted ? <ChevronDown size={ICON_SIZE.xs} /> : <ChevronRight size={ICON_SIZE.xs} />}</span>Completed · {completed.length}
      </button>
      <div id={completedId} hidden={!showCompleted}>{completed.map(row)}</div>
    </div> : null} />;
}

function NoteItemRow({ item, disabled, onChange }: {
  item: NoteItem;
  disabled: boolean;
  onChange(item: NoteItem, patch: { checked?: boolean; task?: boolean }): void;
}) {
  const [expanded, setExpanded] = useState(false);
  return <ContextRow title={item.title} wrapTitle className={`note-item${item.checked ? " context-task-done" : ""}${expanded ? " note-item-expanded" : ""}`}
    control={<input type="checkbox" className={`context-task-checkbox${item.task ? " note-round-checkbox" : ""}`} checked={item.checked} disabled={disabled}
      aria-label={`${item.task ? "Task" : "Checklist"}: ${item.title}`} onChange={() => onChange(item, { checked: !item.checked })} />}
    trailing={<MoreActionsMenu compact label={`Actions for ${item.title}`} actions={[
      { label: expanded ? "Show less text" : "Show full text", onSelect: () => setExpanded(!expanded) },
      { label: `Convert to ${item.task ? "Checklist" : "Task"}`, disabled, onSelect: () => onChange(item, { task: !item.task }) },
    ]} />} />;
}
