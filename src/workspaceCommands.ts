import { useLayoutEffect, useRef } from "react";

export type WorkspaceCommandId = "agents.new" | "agents.search" | "calendar.new" | "calendar.goToday" | "contacts.search" | "contacts.new" | "contacts.newGroup" | "contacts.save" | "files.search" | "files.open" | "files.save" | "files.star" | "today.previousDay" | "today.nextDay" | "notes.next" | "notes.previous" | "notes.new" | "notes.star" | "notes.nextView" | "notes.previousView" | "notes.search" | "notes.trash" | "notes.link" | "notes.pageDown" | "notes.pageUp" | "notes.write" | "notes.previousDay" | "notes.nextDay" | "notes.goToday" | "sidePanel.toggle" | "meetings.prepare" | "meetings.closePrepare" | "meetings.next" | "meetings.previous";
type Actions = Partial<Record<WorkspaceCommandId, () => void>>;
const mountedActions = new Set<{ current: Actions }>();

export function workspaceCommandAvailable(id: WorkspaceCommandId): boolean {
  return [...mountedActions].some((actions) => Boolean(actions.current[id]));
}

const WORKSPACE_COMMAND = "threestrands:workspace-command";

/** The command registry owns bindings; the mounted workspace owns its actions. */
export function runWorkspaceCommand(id: WorkspaceCommandId) {
  window.dispatchEvent(new CustomEvent(WORKSPACE_COMMAND, { detail: id }));
}

export function useWorkspaceCommands(actions: Actions, active = true) {
  const latest = useRef(actions);
  latest.current = actions;
  useLayoutEffect(() => {
    if (!active) return;
    const run = (event: Event) => {
      if (event instanceof CustomEvent) latest.current[event.detail as WorkspaceCommandId]?.();
    };
    mountedActions.add(latest);
    window.addEventListener(WORKSPACE_COMMAND, run);
    return () => { mountedActions.delete(latest); window.removeEventListener(WORKSPACE_COMMAND, run); };
  }, [active]);
}
