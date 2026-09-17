export type FontFamily = string;

const FONT_FAMILY_KEY = "threestrands.settings.fontFamily";
const MAX_FONT_FAMILY_LENGTH = 200;
const LEGACY_FONT_FAMILIES: Record<string, FontFamily> = {
  serif: "Georgia",
  mono: "Menlo",
  "avenir-next": "Avenir Next",
  "helvetica-neue": "Helvetica Neue",
  arial: "Arial",
  georgia: "Georgia",
  "times-new-roman": "Times New Roman",
  verdana: "Verdana",
  menlo: "Menlo",
};

export const DEFAULT_FONT_FAMILY: FontFamily = "system";

export const SYSTEM_FONT_STACK =
  'ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';

function validFontFamily(value: string | null): value is FontFamily {
  if (!value) return false;
  const trimmed = value.trim();
  return trimmed.length > 0
    && trimmed.length <= MAX_FONT_FAMILY_LENGTH
    && !/[\u0000-\u001f\u007f]/.test(trimmed);
}

function quoteCssString(value: string): string {
  return `"${value.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`;
}

export function fontFamilyStack(value: FontFamily): string {
  return value === DEFAULT_FONT_FAMILY
    ? SYSTEM_FONT_STACK
    : `${quoteCssString(value)}, ${SYSTEM_FONT_STACK}`;
}

const AUTO_READ_DELAY_SECONDS_KEY = "threestrands.settings.autoReadDelaySeconds";

const LOAD_REMOTE_IMAGES_KEY = "threestrands.settings.loadRemoteImages";
const SELECTED_ACCOUNT_ID_KEY = "threestrands.settings.selectedAccountId";
const ALL_ACCOUNTS_VALUE = "all";
const MAX_ACCOUNT_ID_LENGTH = 320;

export const DEFAULT_LOAD_REMOTE_IMAGES = false;

export const DEFAULT_AUTO_READ_DELAY_SECONDS = 2;
export const MIN_AUTO_READ_DELAY_SECONDS = 0;
export const MAX_AUTO_READ_DELAY_SECONDS = 60;

export function clampAutoReadDelaySeconds(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_AUTO_READ_DELAY_SECONDS;
  return Math.min(
    MAX_AUTO_READ_DELAY_SECONDS,
    Math.max(MIN_AUTO_READ_DELAY_SECONDS, Math.round(value)),
  );
}

export function readAutoReadDelaySeconds(): number {
  try {
    const saved = localStorage.getItem(AUTO_READ_DELAY_SECONDS_KEY);
    if (saved !== null && saved.trim() !== "") {
      return clampAutoReadDelaySeconds(Number(saved));
    }
  } catch {
    // A blocked storage backend should not prevent the app from opening.
  }
  return DEFAULT_AUTO_READ_DELAY_SECONDS;
}

export function saveAutoReadDelaySeconds(value: number): number {
  const next = clampAutoReadDelaySeconds(value);
  try {
    localStorage.setItem(AUTO_READ_DELAY_SECONDS_KEY, String(next));
  } catch {
    // The preference still applies for this session when storage is unavailable.
  }
  return next;
}

export function readLoadRemoteImages(): boolean {
  try {
    return localStorage.getItem(LOAD_REMOTE_IMAGES_KEY) === "true";
  } catch {
    // A blocked storage backend should not prevent the app from opening.
  }
  return DEFAULT_LOAD_REMOTE_IMAGES;
}

export function saveLoadRemoteImages(value: boolean): boolean {
  try {
    localStorage.setItem(LOAD_REMOTE_IMAGES_KEY, String(value));
  } catch {
    // The preference still applies for this session when storage is unavailable.
  }
  return value;
}

function validAccountId(value: string | null): value is string {
  return value !== null
    && value.length > 0
    && value.length <= MAX_ACCOUNT_ID_LENGTH
    && !/[\u0000-\u001f\u007f]/.test(value);
}

export function readSelectedAccountId(): string | null {
  try {
    const saved = localStorage.getItem(SELECTED_ACCOUNT_ID_KEY);
    if (saved === ALL_ACCOUNTS_VALUE) return null;
    if (validAccountId(saved)) return saved;
  } catch {
    // A blocked storage backend should not prevent the app from opening.
  }
  return null;
}

export function saveSelectedAccountId(value: string | null): string | null {
  const next = validAccountId(value) ? value : null;
  try {
    localStorage.setItem(SELECTED_ACCOUNT_ID_KEY, next ?? ALL_ACCOUNTS_VALUE);
  } catch {
    // The preference still applies for this session when storage is unavailable.
  }
  return next;
}

const SELECTED_TAB_BY_ACCOUNT_KEY = "threestrands.settings.selectedTabByAccount";
const ALL_ACCOUNTS_TAB_KEY = "all";
const MAX_SPLIT_INBOX_ID_LENGTH = 200;

/** Which mailbox tab (Inbox, or a split inbox by id) was last selected, keyed by account email (or "all" for the merged view). */
type SelectedTabByAccount = Record<string, string | null>;

function validSelectedTabByAccount(value: unknown): value is SelectedTabByAccount {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  return Object.entries(value).every(
    ([key, entry]) =>
      key.length > 0
      && key.length <= MAX_ACCOUNT_ID_LENGTH
      && (entry === null
        || (typeof entry === "string" && entry.length > 0 && entry.length <= MAX_SPLIT_INBOX_ID_LENGTH)),
  );
}

/** `null` for a stored entry means the Inbox tab; a missing entry means no preference has been saved yet. */
export function readSelectedTabForAccount(accountId: string | null): string | null | undefined {
  try {
    const saved = localStorage.getItem(SELECTED_TAB_BY_ACCOUNT_KEY);
    if (saved) {
      const parsed: unknown = JSON.parse(saved);
      if (validSelectedTabByAccount(parsed)) {
        const key = accountId ?? ALL_ACCOUNTS_TAB_KEY;
        return Object.hasOwn(parsed, key) ? parsed[key] : undefined;
      }
    }
  } catch {
    // A blocked or corrupted storage backend should not prevent the app from opening.
  }
  return undefined;
}

export function saveSelectedTabForAccount(accountId: string | null, splitInboxId: string | null): void {
  try {
    const saved = localStorage.getItem(SELECTED_TAB_BY_ACCOUNT_KEY);
    const parsed: unknown = saved ? JSON.parse(saved) : {};
    const current = validSelectedTabByAccount(parsed) ? parsed : {};
    const key = accountId ?? ALL_ACCOUNTS_TAB_KEY;
    localStorage.setItem(SELECTED_TAB_BY_ACCOUNT_KEY, JSON.stringify({ ...current, [key]: splitInboxId }));
  } catch {
    // The preference still applies for this session when storage is unavailable.
  }
}

export function readFontFamily(): FontFamily {
  try {
    const saved = localStorage.getItem(FONT_FAMILY_KEY);
    // Preserve preferences saved by the earlier generic and curated selectors.
    if (saved && LEGACY_FONT_FAMILIES[saved]) return LEGACY_FONT_FAMILIES[saved];
    if (validFontFamily(saved)) return saved.trim();
  } catch {
    // A blocked storage backend should not prevent the app from opening.
  }
  return DEFAULT_FONT_FAMILY;
}

export function applyFontFamily(value: FontFamily): void {
  document.documentElement.style.setProperty("--font-family", fontFamilyStack(value));
}

export function saveFontFamily(value: FontFamily): FontFamily {
  const next = validFontFamily(value) ? value.trim() : DEFAULT_FONT_FAMILY;
  applyFontFamily(next);
  try {
    localStorage.setItem(FONT_FAMILY_KEY, next);
  } catch {
    // The preference still applies for this session when storage is unavailable.
  }
  return next;
}
