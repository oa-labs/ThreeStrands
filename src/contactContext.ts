import type { ContactActivity } from "./domain";

/** Rows a context panel section shows before "Show more". */
export const CONTEXT_SECTION_ROWS = 3;
/** A conversation needs at least this many messages for the thread outline. */
export const THREAD_OUTLINE_MIN_MESSAGES = 6;
/** Files fetched for a person; the section reports the full total. */
export const CONTACT_FILE_LIMIT = 50;
/** People and conversations fetched for the same-organization section. */
export const DOMAIN_CONTEXT_LIMIT = 8;
/** Addresses loaded per account for the compose checks; the backend's own maximum. */
export const KNOWN_ADDRESS_LIMIT = 5_000;

/** Messages this close together count as one arrival when estimating cadence. */
const BURST_DAYS = 3;
/** Arrivals needed before claiming a cadence (three gaps). */
const MIN_ARRIVALS = 4;
/** Only the most recent arrivals describe the current habit. */
const CADENCE_WINDOW = 12;
/** Share of gaps that must sit near the median for the pattern to be regular. */
const REGULAR_SHARE = 0.75;
const REGULAR_LOW = 0.6;
const REGULAR_HIGH = 1.4;
/** A cadence lapses once the latest arrival is this many intervals old. */
const LAPSED_INTERVALS = 2.5;
const DAY_MS = 86_400_000;

const CADENCES = [
  { label: "weekly", min: 5, max: 9 },
  { label: "every two weeks", min: 12, max: 17 },
  { label: "monthly", min: 25, max: 35 },
  { label: "quarterly", min: 80, max: 100 },
] as const;

export type Cadence = {
  label: (typeof CADENCES)[number]["label"];
  /** For a monthly habit, where in the month messages usually land. */
  monthPart: "early" | "late" | null;
};

/**
 * Estimates how regularly someone writes from their arrival times. Returns
 * null unless recent arrivals are evenly spaced and still current, so an
 * irregular or lapsed correspondent is never described as a pattern.
 */
export function estimateCadence(receivedAt: string[], now = new Date()): Cadence | null {
  const times = receivedAt.map((iso) => new Date(iso)).filter((date) => !Number.isNaN(date.getTime()))
    .sort((left, right) => left.getTime() - right.getTime());
  const arrivals: Date[] = [];
  for (const time of times) {
    const previous = arrivals.at(-1);
    if (!previous || time.getTime() - previous.getTime() >= BURST_DAYS * DAY_MS) arrivals.push(time);
  }
  const recent = arrivals.slice(-CADENCE_WINDOW);
  if (recent.length < MIN_ARRIVALS) return null;
  const gaps = recent.slice(1).map((time, index) => (time.getTime() - recent[index].getTime()) / DAY_MS);
  const sorted = [...gaps].sort((left, right) => left - right);
  const median = sorted.length % 2 ? sorted[(sorted.length - 1) / 2] : (sorted[sorted.length / 2 - 1] + sorted[sorted.length / 2]) / 2;
  const regular = gaps.filter((gap) => gap >= median * REGULAR_LOW && gap <= median * REGULAR_HIGH).length;
  if (regular / gaps.length < REGULAR_SHARE) return null;
  if ((now.getTime() - recent[recent.length - 1].getTime()) / DAY_MS > median * LAPSED_INTERVALS) return null;
  const cadence = CADENCES.find((candidate) => median >= candidate.min && median <= candidate.max);
  if (!cadence) return null;
  let monthPart: Cadence["monthPart"] = null;
  if (cadence.label === "monthly") {
    const days = recent.map((time) => time.getDate());
    if (days.filter((day) => day <= 10).length / days.length >= REGULAR_SHARE) monthPart = "early";
    else if (days.filter((day) => day >= 20).length / days.length >= REGULAR_SHARE) monthPart = "late";
  }
  return { label: cadence.label, monthPart };
}

const monthYear = new Intl.DateTimeFormat(undefined, { month: "short", year: "numeric" });
const monthDay = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const fullDate = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });

/** "Mar 4" within the current year, otherwise "Mar 2025". */
export function formatHistoryDate(iso: string, now = new Date()): string {
  const date = new Date(iso);
  return date.getFullYear() === now.getFullYear() ? monthDay.format(date) : monthYear.format(date);
}

/**
 * Short facts about the relationship for the contact card, built only from
 * local history: volume and age, any regular cadence, and the user's latest
 * message to the person.
 */
export function describeActivity(activity: ContactActivity, now = new Date()): string[] {
  const total = activity.sentCount + activity.receivedCount;
  if (total === 0 || !activity.firstAt) return [];
  const facts = [total === 1
    ? `1 email · ${fullDate.format(new Date(activity.firstAt))}`
    : `${total} emails since ${monthYear.format(new Date(activity.firstAt))}`];
  const cadence = estimateCadence(activity.recentReceivedAt, now);
  if (cadence) {
    const label = cadence.label[0].toLocaleUpperCase() + cadence.label.slice(1);
    facts.push(cadence.monthPart ? `${label}, usually ${cadence.monthPart} in the month` : label);
  }
  if (activity.lastSentAt && total > 1) facts.push(`You last wrote ${formatHistoryDate(activity.lastSentAt, now)}`);
  return facts;
}

/**
 * Personal mailbox providers, where sharing a domain says nothing about
 * sharing an organization.
 */
const PUBLIC_MAIL_DOMAINS = new Set([
  "aol.com", "fastmail.com", "gmail.com", "gmx.com", "gmx.net", "googlemail.com", "hey.com",
  "hotmail.com", "icloud.com", "live.com", "mac.com", "mail.com", "me.com", "msn.com",
  "outlook.com", "pm.me", "proton.me", "protonmail.com", "tutanota.com", "yahoo.com",
  "yandex.com", "zoho.com",
]);

/** Whether a domain is a personal mailbox provider rather than an organization. */
export function isPersonalMailDomain(domain: string): boolean {
  return PUBLIC_MAIL_DOMAINS.has(domain.trim().toLocaleLowerCase());
}

/**
 * The organization domain to look up colleagues at, or null for personal
 * mailbox providers and for the user's own domains, whose correspondents are
 * the user's colleagues rather than the selected person's.
 */
export function organizationDomain(email: string, ownEmails: string[]): string | null {
  const at = email.lastIndexOf("@");
  if (at <= 0) return null;
  const domain = email.slice(at + 1).trim().toLocaleLowerCase();
  if (!domain.includes(".") || isPersonalMailDomain(domain)) return null;
  if (ownEmails.some((own) => own.toLocaleLowerCase().endsWith(`@${domain}`))) return null;
  return domain;
}
