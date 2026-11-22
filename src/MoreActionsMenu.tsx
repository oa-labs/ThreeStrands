import { Ellipsis } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { ICON_SIZE } from "./iconSizes";
import { useEscapeDismiss } from "./useEscapeDismiss";

export type MoreAction = { label: string; disabled?: boolean; onSelect(): void };

/** A ⋯ button holding the actions a card keeps out of its main row. */
export function MoreActionsMenu({ label, actions, compact = false, onBeforeOpen }: {
  label: string; actions: MoreAction[]; compact?: boolean;
  /** Runs as the user starts to open the menu (pointer down on the trigger, or an arrow key or keyboard click), before opening can move focus or selection. */
  onBeforeOpen?(): void;
}) {
  const [open, setOpen] = useState(false);
  const group = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menuId = useId();
  function close() {
    setOpen(false);
    trigger.current?.focus();
  }
  useEffect(() => {
    if (!open) return;
    function outside(event: PointerEvent) {
      if (!group.current?.contains(event.target as Node)) setOpen(false);
    }
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  if (actions.length === 0) return null;
  // WebKit can blur the focused item without focusing the clicked button.
  // Keep focus in the open menu until the click selects or dismisses it.
  return <div className="more-actions" ref={group} onMouseDown={(event) => {
    if (open && event.button === 0) event.preventDefault();
  }} onBlur={(event) => {
    if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
  }}>
    <button ref={trigger} type="button" className={compact ? "btn-icon btn-icon-sm" : "btn btn-sm"} aria-label={label} title={label} aria-haspopup="menu" aria-expanded={open} aria-controls={open ? menuId : undefined} onPointerDown={() => { if (!open) onBeforeOpen?.(); }} onClick={(event) => { if (!open && event.detail === 0) onBeforeOpen?.(); setOpen((value) => !value); }} onKeyDown={(event) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        if (!open) onBeforeOpen?.();
        setOpen(true);
      }
    }}><Ellipsis size={ICON_SIZE.sm} /></button>
    {open ? <MoreActionsList id={menuId} label={label} actions={actions} onClose={close} /> : null}
  </div>;
}

function MoreActionsList({ id, label, actions, onClose }: { id: string; label: string; actions: MoreAction[]; onClose(): void }) {
  const menu = useRef<HTMLDivElement>(null);
  useEffect(() => { menu.current?.querySelector<HTMLElement>('[role="menuitem"]:not(:disabled)')?.focus(); }, []);
  useEscapeDismiss(onClose);
  return <div ref={menu} id={id} className="more-actions-menu" role="menu" aria-label={label} onKeyDown={(event) => {
    if (event.key === "Escape" && !event.nativeEvent.isComposing) { event.preventDefault(); event.stopPropagation(); onClose(); return; }
    if (event.key === "Tab") { onClose(); return; }
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    event.stopPropagation();
    const items = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)'));
    if (!items.length) return;
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
      : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
    items[next]?.focus();
  }}>
    {actions.map((action) => <button key={action.label} type="button" className="more-actions-item" role="menuitem" disabled={action.disabled} onClick={() => { onClose(); action.onSelect(); }}>{action.label}</button>)}
  </div>;
}
