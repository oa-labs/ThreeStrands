import type { RightWorkspace } from "./useWorkspaces";

// The views the navigation rail switches between; Mail is the absence of one.
// The Today calendar panel beside Mail is left out: it is part of Mail and
// needs a connected calendar account, so launch reopens plain Mail instead.
const RESTORABLE_WORKSPACES: readonly RightWorkspace[] = ["today", "week", "tasks", "contacts", "files"];
const LAST_WORKSPACE_KEY = "threestrands.workspace.last";
const LAUNCH_VIEW_KEY = "threestrands.workspace.launch";

/** What launch opens: the view in use when the app last closed, or Today. */
export type LaunchView = "last" | "today";

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

/** The view to open at launch, from the device's Start With setting. */
export function readLaunchWorkspace(): RightWorkspace {
  return readLaunchView() === "today" ? "today" : readLastWorkspace();
}

export function readLaunchView(): LaunchView {
  try {
    return localStorage.getItem(LAUNCH_VIEW_KEY) === "today" ? "today" : "last";
  } catch {
    return "last";
  }
}

export function writeLaunchView(view: LaunchView) {
  try {
    localStorage.setItem(LAUNCH_VIEW_KEY, view);
  } catch {
    // Launch keeps reopening the last view.
  }
}
