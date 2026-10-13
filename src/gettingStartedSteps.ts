/**
 * The first-run Getting Started checklist. Progress is per device: it lives in
 * localStorage only and is deliberately excluded from settings export and sync.
 */
export const GETTING_STARTED_STORAGE_KEY = "threestrands.gettingStarted";

/** The published product docs. Pages are `docs/<slug>.html` from `site/docs/<slug>.md`. */
export const DOCS_BASE_URL = "https://www.threestrands.app/docs/";

export function docsUrl(slug: string, section?: string): string {
  return `${DOCS_BASE_URL}${slug}.html${section ? `#${section}` : ""}`;
}

export type GettingStartedStepId = "connectMail" | "archive" | "commandPalette" | "shortcuts" | "connectCalendar" | "setUpAi";

/** Steps the app can see directly from its own state, rather than from something the user did. */
export type GettingStartedFacts = { mailConnected: boolean; calendarConnected: boolean; aiReady: boolean };

export type GettingStartedStep = {
  id: GettingStartedStepId;
  title: string;
  description: string;
  actionLabel: string;
  docSlug: string;
  docSection?: string;
  /** Satisfied by app state; the other steps are satisfied by doing them once. */
  fact?: keyof GettingStartedFacts;
  /** Labelled optional, since not everyone wants it. It still counts toward finishing. */
  optional?: boolean;
};

export const GETTING_STARTED_STEPS: GettingStartedStep[] = [
  {
    id: "connectMail",
    title: "Connect a mail account",
    description: "Sign in with Gmail, or add an IMAP account such as Proton Mail Bridge.",
    actionLabel: "Add Account",
    docSlug: "getting-started",
    fact: "mailConnected",
  },
  {
    id: "archive",
    title: "Archive a conversation",
    description: "Press e to mark a conversation done. Shift+E brings it back.",
    actionLabel: "Go to Inbox",
    docSlug: "triage",
  },
  {
    id: "commandPalette",
    title: "Open the command palette",
    description: "Press ⌘K or Ctrl+K to find any command by name.",
    actionLabel: "Open Command Palette",
    docSlug: "keyboard-shortcuts",
  },
  {
    id: "shortcuts",
    title: "Learn the keyboard shortcuts",
    description: "Press ? at any time to see every shortcut.",
    actionLabel: "Show Shortcuts",
    docSlug: "keyboard-shortcuts",
  },
  {
    id: "connectCalendar",
    title: "Connect a calendar",
    description: "See your day beside your mail and create events from conversations.",
    actionLabel: "Connect Calendar",
    docSlug: "calendar",
    fact: "calendarConnected",
    optional: true,
  },
  {
    id: "setUpAi",
    title: "Set up AI assistance",
    description: "Bring your own provider key for summaries, thread chat, and reply drafts.",
    actionLabel: "Set Up AI",
    docSlug: "ai",
    fact: "aiReady",
    optional: true,
  },
];

export type GettingStartedState = { completed: GettingStartedStepId[]; dismissed: boolean };

export const INITIAL_GETTING_STARTED: GettingStartedState = { completed: [], dismissed: false };

const STEP_IDS = new Set<string>(GETTING_STARTED_STEPS.map((step) => step.id));

export function parseGettingStarted(raw: string | null): GettingStartedState {
  if (!raw) return INITIAL_GETTING_STARTED;
  try {
    const value = JSON.parse(raw) as Partial<Record<keyof GettingStartedState, unknown>> | null;
    const completed = Array.isArray(value?.completed)
      ? value.completed.filter((id): id is GettingStartedStepId => typeof id === "string" && STEP_IDS.has(id))
      : [];
    return { completed: [...new Set(completed)], dismissed: value?.dismissed === true };
  } catch {
    return INITIAL_GETTING_STARTED;
  }
}

export function readGettingStarted(): GettingStartedState {
  try {
    return parseGettingStarted(localStorage.getItem(GETTING_STARTED_STORAGE_KEY));
  } catch {
    return INITIAL_GETTING_STARTED;
  }
}

export function saveGettingStarted(state: GettingStartedState): void {
  try {
    localStorage.setItem(GETTING_STARTED_STORAGE_KEY, JSON.stringify(state));
  } catch {
    // Storage can be unavailable; progress still counts for this session.
  }
}

/** Records steps as done. Returns the same object when nothing changed, so callers can skip a save. */
export function completeSteps(state: GettingStartedState, ids: GettingStartedStepId[]): GettingStartedState {
  const added = ids.filter((id) => !state.completed.includes(id));
  return added.length ? { ...state, completed: [...state.completed, ...new Set(added)] } : state;
}

/** Fact-backed steps that the current app state satisfies. */
export function satisfiedFactSteps(facts: GettingStartedFacts): GettingStartedStepId[] {
  return GETTING_STARTED_STEPS.filter((step) => step.fact && facts[step.fact]).map((step) => step.id);
}

export function remainingSteps(state: GettingStartedState): GettingStartedStep[] {
  return GETTING_STARTED_STEPS.filter((step) => !state.completed.includes(step.id));
}

/** The sidebar entry stays until every step is done or the user hides it. */
export function showGettingStarted(state: GettingStartedState): boolean {
  return !state.dismissed && remainingSteps(state).length > 0;
}
