import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { PanelRightClose, PanelRightOpen } from "lucide-react";
import { PanelResizeHandle, useResizableWidth } from "./PanelResizeHandle";
import { focusContextPanel, handleContextPanelKeyDown } from "./contextPanelFocus";
import { ICON_SIZE } from "./iconSizes";

/** Email-derived panel framing; each workspace owns its selection and content. */
export function WorkspaceContextRail({ label, children, onReturn }: { label: string; children: ReactNode; onReturn?(): void }) {
  const panel = useRef<HTMLElement>(null);
  const storageKey = `threestrands.workspaceContext.collapsed.${label}`;
  const [collapsed, setCollapsed] = useState(() => { try { return localStorage.getItem(storageKey)==="true"; } catch { return false; } });
  useEffect(() => { try { localStorage.setItem(storageKey,String(collapsed)); } catch { /* Layout remains available without storage. */ } }, [collapsed,storageKey]);
  useEffect(() => {
    const focus = (event: Event) => {
      if (!(event instanceof CustomEvent) || event.detail!==label) return;
      setCollapsed(false);
      requestAnimationFrame(() => { if (panel.current) focusContextPanel(panel.current); });
    };
    window.addEventListener("threestrands:focus-context",focus);
    return () => window.removeEventListener("threestrands:focus-context",focus);
  }, [label]);
  const size = useResizableWidth({ storageKey: "threestrands.workspaceContextWidth", minimumWidth: 300, defaultWidth: 340, maximumWidth: 520, reservedWidth: 58 + 626 });
  return <div className={`workspace-context-container${collapsed ? " collapsed" : ""}`} style={{ "--workspace-context-width": `${size.width}px` } as CSSProperties}>
    <button type="button" className="btn-icon workspace-context-toggle" aria-label={collapsed ? `Show ${label}` : `Hide ${label}`} aria-expanded={!collapsed} aria-controls={`workspace-${label.replaceAll(" ", "-")}`} onClick={() => setCollapsed((value) => !value)}>
      {collapsed ? <PanelRightOpen size={ICON_SIZE.sm} /> : <PanelRightClose size={ICON_SIZE.sm} />}
    </button>
    <>
      <PanelResizeHandle {...size} panelSide="right" label={`Resize ${label}`} controlsId={`workspace-${label.replaceAll(" ", "-")}`} title="Resize context panel" />
      <aside ref={panel} hidden={collapsed} id={`workspace-${label.replaceAll(" ", "-")}`} className="context-panel workspace-context-rail" aria-label={label} tabIndex={-1}
        onKeyDown={(event) => handleContextPanelKeyDown(event, () => { if (window.innerWidth<=900) setCollapsed(true); onReturn?.(); })}>{children}</aside>
    </>
  </div>;
}
