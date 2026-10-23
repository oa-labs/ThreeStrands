import type { RightWorkspace } from "./useWorkspaces";

// The views the navigation rail switches between; Mail is the absence of one.
const RESTORABLE_WORKSPACES: readonly RightWorkspace[] = ["today", "week", "tasks", "contacts", "files"];
const LAST_WORKSPACE_KEY = "threestrands.workspace.last";
const LAUNCH_VIEW_KEY = "threestrands.workspace.launch";
const SIDE_COLUMN_KEY = "threestrands.workspace.sideColumn";

/** What Mail's right column shows: the conversation context, or the day's schedule. */
export type SideColumn = "context" | "schedule";

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

/** The right column chosen last, kept across views and launches. */
export function readSideColumn(): SideColumn {
  try {
    return localStorage.getItem(SIDE_COLUMN_KEY) === "schedule" ? "schedule" : "context";
  } catch {
    return "context";
  }
}

export function writeSideColumn(column: SideColumn) {
  try {
    localStorage.setItem(SIDE_COLUMN_KEY, column);
  } catch {
    // The column still applies for this session; launch shows the context.
  }
}
