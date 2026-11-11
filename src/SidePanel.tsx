import { useCallback } from "react";
import { PanelRightOpen } from "lucide-react";
import { readLocalStorage, useLocalStorageState } from "./useLocalStorageState";
import { PanelResizeHandle, useResizableWidth } from "./PanelResizeHandle";
import { useWorkspaceCommands } from "./workspaceCommands";
import { formatShortcut } from "./shortcutLabels";
import { ICON_SIZE } from "./iconSizes";

/** Shared by every side panel, so a width chosen in one view holds in the others. */
export const SIDE_PANEL_WIDTH_KEY = "threestrands.workspaceContextWidth";
/** Below this share of the minimum width, a drag hides the panel instead of narrowing it. */
const COLLAPSE_AT = 0.5;

/** Whether `view`'s side panel is hidden, remembered per view on this device. */
const collapsedKey = (view: string) => `threestrands.workspaceContext.collapsed.${view}`;

/**
 * A view's side panel: its width (shared by every view), whether it is
 * hidden (per view), and Toggle Side Panel (`\`) while `active`. Hiding the
 * panel while focus is inside it calls `onReturn` so focus goes back to the
 * view. A view that handles the command itself, such as Mail with its
 * narrow-window drawer, passes `toggleCommand: false`.
 */
export function useSidePanel({ view, panel, onReturn, active = true, toggleCommand = true }: {
  view: string;
  /** The panel element, to tell whether focus is inside it. */
  panel(): HTMLElement | null;
  onReturn?(): void;
  active?: boolean;
  toggleCommand?: boolean;
}) {
  const key = collapsedKey(view);
  const [collapsed, setCollapsed] = useLocalStorageState(key, () => readLocalStorage(key, (raw) => raw === "true", false));
  const setHidden = useCallback((hide: boolean) => {
    const element = panel();
    if (hide && element?.contains(document.activeElement)) {
      if (onReturn) onReturn();
      else (document.activeElement as HTMLElement | null)?.blur();
    }
    setCollapsed(hide);
  }, [panel, onReturn, setCollapsed]);
  useWorkspaceCommands({ "sidePanel.toggle": () => setHidden(!collapsed) }, active && toggleCommand);
  const size = useResizableWidth({ storageKey: SIDE_PANEL_WIDTH_KEY, minimumWidth: 300, defaultWidth: 340, maximumWidth: 520, reservedWidth: 58 + 626 });
  return { collapsed, setCollapsed, setHidden, size, threshold: Math.round(size.minWidth * COLLAPSE_AT) };
}

export type SidePanel = ReturnType<typeof useSidePanel>;

/**
 * The divider on a side panel's left edge, and, while the panel is hidden,
 * the strip's button that shows it. The caller places them beside the panel.
 */
export function SidePanelControls({ panel, label, controlsId }: {
  panel: SidePanel;
  /** The panel's name, as in "Show Note context". */
  label: string;
  controlsId: string;
}) {
  const shortcut = formatShortcut("\\");
  return <>
    <div className="side-panel-edge">
      <PanelResizeHandle {...panel.size} panelSide="right" label={`Resize ${label}`} controlsId={controlsId}
        title={`Drag to resize the side panel, or past its edge to hide it (${shortcut}). Use arrow keys to adjust; double-click to reset.`}
        collapse={{ collapsed: panel.collapsed, threshold: panel.threshold, onChange: panel.setHidden }} />
    </div>
    {panel.collapsed ? <div className="side-panel-strip">
      <button type="button" className="btn-icon" aria-label={`Show ${label}`} title={`Show ${label} (${shortcut})`} aria-keyshortcuts="\" aria-expanded={false} aria-controls={controlsId} onClick={() => panel.setHidden(false)}>
        <PanelRightOpen size={ICON_SIZE.lg} />
      </button>
    </div> : null}
  </>;
}
