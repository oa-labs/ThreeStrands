/**
 * Pure search-box autocomplete: finds the token under the caret, offers
 * operator names, operator values, and recent searches, and applies a chosen
 * suggestion back into the query. Contacts are fetched by the caller and
 * passed in, so everything here is synchronous and table-testable.
 *
 * The operators mirror the ones the shared query parser understands
 * (`crates/mail-text/src/search_query.rs`); offering anything else would
 * suggest syntax that silently falls back to plain-text search.
 */

export type SearchToken = {
  /** Offsets of the token in the query; `end` is exclusive. */
  start: number;
  end: number;
  /** A leading `-`: the term is excluded, and suggestions keep the `-`. */
  negated: boolean;
  /** The lowercased operator name before `:`, or null for a bare word. */
  operator: string | null;
  /** The text after `operator:` with quotes removed, or the bare word. */
  value: string;
};

export type SearchSuggestionKind = "operator" | "contact" | "label" | "location" | "value" | "date" | "recent";

export type SearchSuggestion = {
  key: string;
  kind: SearchSuggestionKind;
  label: string;
  detail?: string;
  /** Text that replaces the token (or, for a recent search, the whole query). */
  replacement: string;
  /** A finished term gets a trailing space; an operator name keeps the caret after `:`. */
  complete: boolean;
};

/**
 * A person to search for. `addresses` lists every address to match (the
 * suggested one first); a contact with several becomes `{from:a from:b}`.
 */
export type SuggestionContact = { email: string; name: string | null; addresses?: readonly string[] };

export type SuggestionSources = {
  /** User label names the active account(s) know about. */
  labels: readonly string[];
  /** Contacts matching the current `from:`/`to:` value, best first. */
  contacts: readonly SuggestionContact[];
  recentSearches: readonly string[];
  now: Date;
};

export const MAX_SEARCH_SUGGESTIONS = 8;

const OPERATORS: readonly { text: string; detail: string; complete: boolean }[] = [
  { text: "from:", detail: "Sender", complete: false },
  { text: "to:", detail: "Recipient", complete: false },
  { text: "subject:", detail: "Words in the subject", complete: false },
  { text: "label:", detail: "Label", complete: false },
  { text: "in:", detail: "Folder or location", complete: false },
  { text: "is:unread", detail: "Unread", complete: true },
  { text: "is:starred", detail: "Starred", complete: true },
  { text: "has:attachment", detail: "Has attachments", complete: true },
  { text: "after:", detail: "On or after a date", complete: false },
  { text: "before:", detail: "Before a date", complete: false },
];

const KNOWN_OPERATORS = new Set(["from", "to", "subject", "label", "in", "is", "has", "after", "before"]);

const LOCATIONS: readonly { word: string; detail: string }[] = [
  { word: "inbox", detail: "Inbox" },
  { word: "sent", detail: "Sent" },
  { word: "archive", detail: "Archived mail" },
  { word: "trash", detail: "Trash" },
  { word: "spam", detail: "Spam" },
  { word: "anywhere", detail: "Everywhere, including Trash and Spam" },
];

const STATE_VALUES: Record<string, readonly { word: string; detail: string }[]> = {
  is: [{ word: "unread", detail: "Unread" }, { word: "starred", detail: "Starred" }],
  has: [{ word: "attachment", detail: "Has attachments" }],
};

/** The token the caret is in, or an empty token at the caret between words. */
export function tokenAt(query: string, caret: number): SearchToken {
  const position = Math.max(0, Math.min(caret, query.length));
  let start = 0;
  let inQuotes = false;
  for (let index = 0; index < position; index += 1) {
    const char = query[index];
    if (char === "\"") inQuotes = !inQuotes;
    else if (!inQuotes && /\s/.test(char)) start = index + 1;
  }
  let end = position;
  for (; end < query.length; end += 1) {
    const char = query[end];
    if (char === "\"") inQuotes = !inQuotes;
    else if (!inQuotes && /\s/.test(char)) break;
  }
  const text = query.slice(start, end);
  const negated = text.length > 1 && text.startsWith("-");
  const term = negated ? text.slice(1) : text;
  const match = /^([a-z]+):(.*)$/i.exec(term);
  if (match && KNOWN_OPERATORS.has(match[1].toLowerCase())) {
    return { start, end, negated, operator: match[1].toLowerCase(), value: match[2].replaceAll("\"", "") };
  }
  return { start, end, negated, operator: null, value: term };
}

/** An operator value, quoted when it holds whitespace so it stays one term. */
export function formatOperatorValue(value: string): string {
  return /\s/.test(value) ? `"${value.replaceAll("\"", "")}"` : value;
}

function startsWithFold(candidate: string, prefix: string) {
  return candidate.toLocaleLowerCase().startsWith(prefix.toLocaleLowerCase());
}

function includesFold(candidate: string, needle: string) {
  return candidate.toLocaleLowerCase().includes(needle.toLocaleLowerCase());
}

/** `YYYY/MM/DD` in local time, the date form the query parser accepts. */
export function formatSearchDate(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())}`;
}

function shiftDate(now: Date, { days = 0, months = 0, years = 0 }) {
  const shifted = new Date(now.getFullYear() + years, now.getMonth() + months, now.getDate() + days);
  // A month or year shift from a day the target month lacks (the 31st, or
  // February 29) rolls into the next month; pin it to the target month's end.
  if ((months || years) && shifted.getDate() !== now.getDate()) shifted.setDate(0);
  return shifted;
}

function datePresets(operator: "after" | "before", now: Date) {
  const presets = [
    { label: operator === "after" ? "Past week" : "Older than a week", date: shiftDate(now, { days: -7 }) },
    { label: operator === "after" ? "Past month" : "Older than a month", date: shiftDate(now, { months: -1 }) },
    { label: operator === "after" ? "Past 3 months" : "Older than 3 months", date: shiftDate(now, { months: -3 }) },
    { label: operator === "after" ? "Past year" : "Older than a year", date: shiftDate(now, { years: -1 }) },
  ];
  return presets.map(({ label, date }) => ({ label, value: formatSearchDate(date) }));
}

function uniqueFold(values: readonly string[]) {
  const seen = new Set<string>();
  return values.filter((value) => {
    const key = value.toLocaleLowerCase();
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** Whether the token is a `from:`/`to:` value the caller should fetch contacts for. */
export function contactQueryFor(token: SearchToken): string | null {
  return token.operator === "from" || token.operator === "to" ? token.value : null;
}

/**
 * Suggestions for the token under the caret. An empty query offers recent
 * searches; a bare word offers matching operator names; an operator offers
 * its values.
 */
export function searchSuggestions(query: string, token: SearchToken, sources: SuggestionSources): SearchSuggestion[] {
  if (!query.trim()) {
    return sources.recentSearches.slice(0, MAX_SEARCH_SUGGESTIONS).map((recent) => ({
      key: `recent:${recent}`,
      kind: "recent",
      label: recent,
      replacement: recent,
      complete: true,
    }));
  }

  const suggestions: SearchSuggestion[] = [];
  const prefix = token.negated ? "-" : "";
  const valueSuggestion = (kind: SearchSuggestionKind, value: string, label: string, detail?: string): SearchSuggestion => ({
    key: `${kind}:${token.operator}:${value}`,
    kind,
    label,
    detail,
    replacement: `${prefix}${token.operator}:${formatOperatorValue(value)}`,
    complete: true,
  });

  switch (token.operator) {
    case null: {
      if (!token.value) break;
      for (const operator of OPERATORS) {
        if (startsWithFold(operator.text, token.value) && operator.text.toLowerCase() !== token.value.toLowerCase()) {
          suggestions.push({
            key: `operator:${operator.text}`,
            kind: "operator",
            label: operator.text,
            detail: operator.detail,
            replacement: `${prefix}${operator.text}`,
            complete: operator.complete,
          });
        }
      }
      break;
    }
    case "from":
    case "to":
      for (const contact of sources.contacts) {
        const addresses = contact.addresses?.length ? contact.addresses : [contact.email];
        if (addresses.length === 1) {
          suggestions.push(valueSuggestion("contact", contact.email, contact.name || contact.email, contact.name ? contact.email : undefined));
          continue;
        }
        // Every address the person uses, as alternatives: `{from:a from:b}`.
        // Negated, the query parser reads `-{…}` as "none of them".
        suggestions.push({
          key: `contact:${token.operator}:${addresses.join(",")}`,
          kind: "contact",
          label: contact.name || contact.email,
          detail: `${addresses[0]} +${addresses.length - 1}`,
          replacement: `${prefix}{${addresses.map((address) => `${token.operator}:${formatOperatorValue(address)}`).join(" ")}}`,
          complete: true,
        });
      }
      break;
    case "label":
      for (const name of uniqueFold(sources.labels)) {
        if (includesFold(name, token.value)) suggestions.push(valueSuggestion("label", name, name));
      }
      break;
    case "in":
      for (const location of LOCATIONS) {
        if (startsWithFold(location.word, token.value)) {
          suggestions.push(valueSuggestion("location", location.word, location.word, location.detail));
        }
      }
      for (const name of uniqueFold(sources.labels)) {
        if (includesFold(name, token.value)) suggestions.push(valueSuggestion("label", name, name, "Label"));
      }
      break;
    case "is":
    case "has":
      for (const state of STATE_VALUES[token.operator]) {
        if (startsWithFold(state.word, token.value)) {
          suggestions.push(valueSuggestion("value", state.word, state.word, state.detail));
        }
      }
      break;
    case "after":
    case "before":
      for (const preset of datePresets(token.operator, sources.now)) {
        if (preset.value.startsWith(token.value)) {
          suggestions.push(valueSuggestion("date", preset.value, preset.label, preset.value));
        }
      }
      break;
    default:
      break;
  }

  // A value the user already typed in full is not worth suggesting back.
  const typed = query.slice(token.start, token.end).toLocaleLowerCase();
  return suggestions
    .filter((suggestion) => suggestion.replacement.toLocaleLowerCase() !== typed)
    .slice(0, MAX_SEARCH_SUGGESTIONS);
}

/** Puts a chosen suggestion into the query and returns the new text and caret. */
export function applySuggestion(query: string, token: SearchToken, suggestion: SearchSuggestion): { query: string; caret: number } {
  if (suggestion.kind === "recent") return { query: suggestion.replacement, caret: suggestion.replacement.length };
  const before = query.slice(0, token.start);
  const after = query.slice(token.end);
  const insertion = suggestion.complete && !after.startsWith(" ") ? `${suggestion.replacement} ` : suggestion.replacement;
  return { query: `${before}${insertion}${after}`, caret: before.length + insertion.length + (suggestion.complete && after.startsWith(" ") ? 1 : 0) };
}
