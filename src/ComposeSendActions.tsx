import { ChevronDown, Send } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { ICON_SIZE } from "./iconSizes";
import { composeKeyLabel } from "./richText";
import { useEscapeDismiss } from "./useEscapeDismiss";

export function ComposeSendActions({ busy, onSend, onSchedule, onSendAndArchive }: {
  busy: boolean;
  onSend(): void;
  onSchedule(): void;
  onSendAndArchive?: () => void;
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
  return <div className="compose-send-actions" ref={group} onKeyDown={(event) => {
    // Modified Enter must send rather than activate the focused menu item.
    if (event.nativeEvent.isComposing || event.key !== "Enter" || !(event.metaKey || event.ctrlKey)) return;
    event.preventDefault();
    event.stopPropagation();
    if (busy || event.repeat || event.altKey) return;
    close();
    if (event.shiftKey) onSendAndArchive?.();
    else onSend();
  }} onBlur={(event) => {
    if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
  }}>
    <button type="button" className="btn btn-primary send-button" aria-label="Send" aria-keyshortcuts="Meta+Enter Control+Enter" title={`Send (${composeKeyLabel("Mod+Enter")})`} onClick={onSend} disabled={busy}>
      <Send size={ICON_SIZE.md} /> Send <kbd aria-hidden="true">{composeKeyLabel("Mod+Enter")}</kbd>
    </button>
    <button ref={trigger} type="button" className="btn btn-primary compose-send-options" aria-label="Send options" aria-haspopup="menu" aria-expanded={open && !busy} aria-controls={open && !busy ? menuId : undefined} title="Send options" disabled={busy} onClick={() => setOpen((value) => !value)} onKeyDown={(event) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setOpen(true);
      }
    }}><ChevronDown size={ICON_SIZE.sm} /></button>
    {open && !busy ? <SendOptionsMenu id={menuId} onClose={close} onSchedule={() => { close(); onSchedule(); }} onSendAndArchive={onSendAndArchive ? () => { close(); onSendAndArchive(); } : undefined} /> : null}
  </div>;
}

function SendOptionsMenu({ id, onClose, onSchedule, onSendAndArchive }: {
  id: string;
  onClose(): void;
  onSchedule(): void;
  onSendAndArchive?: () => void;
}) {
  const menu = useRef<HTMLDivElement>(null);
  useEffect(() => { menu.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus(); }, []);
  useEscapeDismiss(onClose);
  return <div ref={menu} id={id} className="compose-send-menu" role="menu" aria-label="Send options" onKeyDown={(event) => {
    if (event.key === "Tab") { onClose(); return; }
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    event.stopPropagation();
    const items = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'));
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
      : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
    items[next]?.focus();
  }}>
    <button type="button" className="compose-send-menu-item" role="menuitem" aria-label="Send later…" aria-keyshortcuts="Meta+Shift+L Control+Shift+L" onClick={onSchedule}>
      Send later… <kbd aria-hidden="true">{composeKeyLabel("Mod+Shift+L")}</kbd>
    </button>
    {onSendAndArchive ? <button type="button" className="compose-send-menu-item" role="menuitem" aria-label="Send & archive" aria-keyshortcuts="Meta+Shift+Enter Control+Shift+Enter" onClick={onSendAndArchive}>
      Send &amp; archive <kbd aria-hidden="true">{composeKeyLabel("Mod+Shift+Enter")}</kbd>
    </button> : null}
  </div>;
}
