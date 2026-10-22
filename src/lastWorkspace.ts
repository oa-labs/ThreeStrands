import type { RightWorkspace } from "./useWorkspaces";

// The views the navigation rail switches between; Mail is the absence of one.
// The Today calendar panel beside Mail is left out: it is part of Mail and
// needs a connected calendar account, so launch reopens plain Mail instead.
const RESTORABLE_WORKSPACES: readonly RightWorkspace[] = ["week", "tasks", "contacts", "files"];
const LAST_WORKSPACE_KEY = "threestrands.workspace.last";

export function readLastWorkspace(): RightWorkspace {
  try {
    const stored = localStorage.getItem(LAST_WORKSPACE_KEY);
    return RESTORABLE_WORKSPACES.find((name) => name === stored) ?? null;
  } catch {
    return null;
  }
}

export function writeLastWorkspace(workspace: RightWorkspace) {
  try {
    const restorable = RESTORABLE_WORKSPACES.includes(workspace) ? workspace : null;
    localStorage.setItem(LAST_WORKSPACE_KEY, restorable ?? "mail");
  } catch {
    // The view still applies for this session; launch falls back to Mail.
  }
}
