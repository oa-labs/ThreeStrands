import { useLayoutEffect, useRef } from "react";

export type WorkspaceCommandId = "calendar.new" | "calendar.goToday" | "contacts.search" | "contacts.new" | "contacts.newGroup" | "contacts.save" | "files.search" | "files.open" | "files.save" | "files.star";
const WORKSPACE_COMMAND = "threestrands:workspace-command";

/** The command registry owns bindings; the mounted workspace owns its actions. */
export function runWorkspaceCommand(id: WorkspaceCommandId) {
  window.dispatchEvent(new CustomEvent(WORKSPACE_COMMAND, { detail: id }));
}

export function useWorkspaceCommands(actions: Partial<Record<WorkspaceCommandId, () => void>>, active = true) {
  const latest = useRef(actions);
  latest.current = actions;
  useLayoutEffect(() => {
    if (!active) return;
    const run = (event: Event) => {
      if (event instanceof CustomEvent) latest.current[event.detail as WorkspaceCommandId]?.();
    };
    window.addEventListener(WORKSPACE_COMMAND, run);
    return () => window.removeEventListener(WORKSPACE_COMMAND, run);
  }, [active]);
}
