import { readLocalStorage, useLocalStorageState } from "./useLocalStorageState";
import { useEffect, useRef, type CSSProperties, type ReactNode } from "react";
import { PanelRightClose, PanelRightOpen } from "lucide-react";
import { PanelResizeHandle, useResizableWidth } from "./PanelResizeHandle";
import { focusContextPanel, handleContextPanelKeyDown } from "./contextPanelFocus";
import { ICON_SIZE } from "./iconSizes";
import { useWorkspaceCommands } from "./workspaceCommands";
import { formatShortcut } from "./shortcutLabels";

/** Shared by every side panel, so a width chosen in one view holds in the others. */
export const SIDE_PANEL_WIDTH_KEY = "threestrands.workspaceContextWidth";
/** Below this share of the minimum width, a drag hides the panel instead of narrowing it. */
const COLLAPSE_AT = 0.5;
const NARROW_LAYOUT = 900;

/**
 * Email-derived panel framing; each workspace owns its selection and content.
 * The panel resizes from its left edge; dragging it well past its narrowest
 * width hides it, leaving a strip whose button (or a drag back out) shows it
 * again. Toggle Side Panel (`\`) does the same from the keyboard. The width
 * is shared by every view and the hidden state is remembered per view. In a
 * narrow window the panel opens over the view, without a divider, and closes
 * from its own button or Escape.
 */
export function WorkspaceContextRail({ label, children, onReturn }: { label: string; children: ReactNode; onReturn?(): void }) {
  const panel = useRef<HTMLElement>(null);
  const storageKey = `threestrands.workspaceContext.collapsed.${label}`;
  const [collapsed, setCollapsed] = useLocalStorageState(storageKey, () => readLocalStorage(storageKey, (raw) => raw === "true", false));
  useEffect(() => {
    const focus = (event: Event) => {
      if (!(event instanceof CustomEvent) || event.detail!==label) return;
      setCollapsed(false);
      requestAnimationFrame(() => { if (panel.current) focusContextPanel(panel.current); });
    };
    window.addEventListener("threestrands:focus-context",focus);
    return () => window.removeEventListener("threestrands:focus-context",focus);
  }, [label, setCollapsed]);
  /** Hides or shows the panel; hiding it with focus inside returns focus to the view. */
  const setHidden = (hide: boolean) => {
    if (hide && panel.current?.contains(document.activeElement)) onReturn?.();
    setCollapsed(hide);
  };
  useWorkspaceCommands({ "sidePanel.toggle": () => setHidden(!collapsed) });
  const size = useResizableWidth({ storageKey: SIDE_PANEL_WIDTH_KEY, minimumWidth: 300, defaultWidth: 340, maximumWidth: 520, reservedWidth: 58 + 626 });
  const id = `workspace-${label.replaceAll(" ", "-")}`;
  const shortcut = formatShortcut("\\");
  return <div className={`workspace-context-container${collapsed ? " collapsed" : ""}`} style={{ "--workspace-context-width": `${size.width}px` } as CSSProperties}>
    {collapsed
      ? <button type="button" className="btn-icon workspace-context-toggle" aria-label={`Show ${label}`} title={`Show ${label} (${shortcut})`} aria-keyshortcuts="\" aria-expanded={false} aria-controls={id} onClick={() => setHidden(false)}>
        <PanelRightOpen size={ICON_SIZE.lg} />
      </button>
      // A narrow window has no divider to drag, so there the open panel keeps a close button.
      : <button type="button" className="btn-icon workspace-context-toggle workspace-context-close" aria-label={`Hide ${label}`} title={`Hide ${label} (${shortcut})`} aria-keyshortcuts="\" aria-expanded aria-controls={id} onClick={() => setHidden(true)}>
        <PanelRightClose size={ICON_SIZE.lg} />
      </button>}
    <PanelResizeHandle {...size} panelSide="right" label={`Resize ${label}`} controlsId={id}
      title={`Drag to resize the side panel, or past its edge to hide it (${shortcut}). Use arrow keys to adjust; double-click to reset.`}
      collapse={{ collapsed, threshold: Math.round(size.minWidth * COLLAPSE_AT), onChange: setHidden }} />
    <aside ref={panel} hidden={collapsed} id={id} className="context-panel workspace-context-rail" aria-label={label} tabIndex={-1}
      onKeyDown={(event) => handleContextPanelKeyDown(event, () => { if (window.innerWidth<=NARROW_LAYOUT) setCollapsed(true); onReturn?.(); })}>{children}</aside>
  </div>;
}
