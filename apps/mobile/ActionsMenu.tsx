import { useEffect, useRef, useState, type ReactNode } from "react";
import { MoreHorizontal } from "lucide-react";

export function ActionsMenu({ label, children }: { label: string; children: ReactNode }) {
  const menu = useRef<HTMLDetailsElement>(null);
  const trigger = useRef<HTMLElement>(null);
  const [open, setOpen] = useState(false);

  function close(returnFocus = false) {
    menu.current?.removeAttribute("open");
    if (returnFocus) trigger.current?.focus();
  }

  useEffect(() => {
    if (!open) return;
    function dismiss(event: PointerEvent) {
      if (event.target instanceof Node && !menu.current?.contains(event.target)) close();
    }
    document.addEventListener("pointerdown", dismiss);
    return () => document.removeEventListener("pointerdown", dismiss);
  }, [open]);

  return <details className="action-menu" ref={menu} onToggle={event => setOpen(event.currentTarget.open)}
    onKeyDown={event => {
      if (event.key !== "Escape" || !event.currentTarget.open) return;
      event.stopPropagation();
      close(true);
    }}>
    <summary ref={trigger} aria-label={label} title={label}><MoreHorizontal size={21} aria-hidden="true" /></summary>
    <div className="action-menu-panel" onClick={event => {
      if (event.target instanceof Element && event.target.closest("button:enabled")) close(true);
    }}>{children}</div>
  </details>;
}
