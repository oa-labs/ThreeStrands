import type { Draft } from "./correspondence";
import type { ContactGroupRecipients, ContactSuggestion } from "./domain";
import { overexposedGroups } from "./contactGroups";
import { isPersonalMailDomain } from "./contactContext";
import { looksLikeCompleteAddress, parseAddress, splitAddressList } from "./emailAddress";

/** One complete address on a draft, in the first field it appears in. */
export type DraftRecipient = { email: string; name: string | null; field: "to" | "cc" | "bcc" };

/**
 * The complete addresses on a draft, To first, without duplicates or the
 * user's own addresses. A segment still being typed is left out so the panel
 * does not chase every keystroke.
 */
export function draftRecipients(draft: Pick<Draft, "to" | "cc" | "bcc">, ownEmails: string[]): DraftRecipient[] {
  const own = new Set(ownEmails.map((email) => email.toLocaleLowerCase()));
  const seen = new Set<string>();
  const recipients: DraftRecipient[] = [];
  for (const field of ["to", "cc", "bcc"] as const) {
    for (const segment of splitAddressList(draft[field])) {
      const parsed = parseAddress(segment);
      const email = parsed.email.trim().toLocaleLowerCase();
      if (!looksLikeCompleteAddress(email) || own.has(email) || seen.has(email)) continue;
      seen.add(email);
      recipients.push({ email, name: parsed.name && parsed.name !== parsed.email ? parsed.name : null, field });
    }
  }
  return recipients;
}

/** Correspondence with one address in one account, from contact suggestions. */
export type KnownAddress = { sent: number; received: number; displayName: string | null; pinned: boolean };
/** Known addresses per account email, each keyed by lowercase address. */
export type KnownCorrespondents = Record<string, Map<string, KnownAddress>>;

export function knownAddressMap(suggestions: ContactSuggestion[]): Map<string, KnownAddress> {
  return new Map(suggestions.map((item) => [item.email.toLocaleLowerCase(), {
    sent: item.sentCount,
    received: item.receivedCount,
    displayName: item.displayName,
    pinned: item.pinned,
  }]));
}

export type ComposeCheck =
  | { kind: "attachment"; word: string }
  | { kind: "subject" }
  | { kind: "typo"; email: string; suggestion: string; suggestionName: string | null }
  | { kind: "firstContact"; email: string }
  | { kind: "outside"; domain: string; emails: string[] }
  | { kind: "account"; account: string; emails: string[] }
  | { kind: "groupExposure"; group: string; count: number; emails: string[] };

/** Words that promise an attachment. "Attachment" alone is left out: replies discuss the sender's. */
const ATTACHMENT_WORDS = /\b(attached|attaching|enclosed)\b/i;
const REPLY_QUOTE = /\n\nOn [\s\S]*? wrote:\n/;
const FORWARD_MARKER = "\n\n---------- Forwarded message ----------\n";

/** The part of a draft body the user wrote, before any quoted or forwarded message. */
export function authoredText(body: string): string {
  const quote = body.search(REPLY_QUOTE);
  const forward = body.indexOf(FORWARD_MARKER);
  const ends = [quote, forward].filter((index) => index >= 0);
  return ends.length ? body.slice(0, Math.min(...ends)) : body;
}

/**
 * Optimal string alignment distance, capped: returns `limit + 1` as soon as
 * the distance must exceed `limit`. Adjacent swaps count as one edit, the
 * most common typing slip.
 */
export function editDistance(left: string, right: string, limit: number): number {
  if (Math.abs(left.length - right.length) > limit) return limit + 1;
  let before: number[] = [];
  let previous = Array.from({ length: right.length + 1 }, (_, index) => index);
  for (let i = 1; i <= left.length; i++) {
    const current = [i];
    let rowMinimum = i;
    for (let j = 1; j <= right.length; j++) {
      const cost = left[i - 1] === right[j - 1] ? 0 : 1;
      let value = Math.min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + cost);
      if (i > 1 && j > 1 && left[i - 1] === right[j - 2] && left[i - 2] === right[j - 1]) {
        value = Math.min(value, before[j - 2] + 1);
      }
      current.push(value);
      rowMinimum = Math.min(rowMinimum, value);
    }
    if (rowMinimum > limit) return limit + 1;
    before = previous;
    previous = current;
  }
  return Math.min(previous[right.length], limit + 1);
}

function splitEmail(email: string): [string, string] {
  const at = email.lastIndexOf("@");
  return [email.slice(0, at), email.slice(at + 1)];
}

/** Local parts this long may differ by two edits; shorter names collide too easily. */
const LONG_LOCAL_PART = 8;
/** Domains may differ by this many edits (gmial.com, acme.co). */
const DOMAIN_TYPO_EDITS = 2;

/**
 * An address the user has written to that is a likely intended spelling of
 * `email`: same domain with a near local part, or same local part at a near
 * domain. Only addresses the user has sent to (or pinned/favorited) count, so
 * a stranger's similar address is never suggested.
 */
export function likelyIntendedAddress(email: string, known: Iterable<[string, KnownAddress]>): { email: string; name: string | null } | null {
  const [local, domain] = splitEmail(email);
  let best: { email: string; name: string | null; distance: number; sent: number } | null = null;
  for (const [candidate, history] of known) {
    if (candidate === email || (history.sent === 0 && !history.pinned)) continue;
    const [candidateLocal, candidateDomain] = splitEmail(candidate);
    let distance: number;
    if (candidateDomain === domain) {
      const limit = Math.min(local.length, candidateLocal.length) >= LONG_LOCAL_PART ? 2 : 1;
      distance = editDistance(local, candidateLocal, limit);
      if (distance > limit) continue;
    } else if (candidateLocal === local) {
      distance = editDistance(domain, candidateDomain, DOMAIN_TYPO_EDITS);
      if (distance > DOMAIN_TYPO_EDITS) continue;
    } else {
      continue;
    }
    if (!best || distance < best.distance || (distance === best.distance && history.sent > best.sent)) {
      best = { email: candidate, name: history.displayName, distance, sent: history.sent };
    }
  }
  return best ? { email: best.email, name: best.name } : null;
}

function domainOf(email: string): string {
  return splitEmail(email)[1].toLocaleLowerCase();
}

/**
 * Mistakes worth a second look before a draft is sent. Every check is local
 * and deterministic, and none blocks sending. `known` is null until loaded,
 * which leaves out the checks that depend on correspondence history; `groups`
 * likewise leaves out the group check.
 */
export function composeChecks({ draft, ownEmails, known, groups = null }: {
  draft: Pick<Draft, "to" | "cc" | "bcc" | "subject" | "body" | "attachments" | "account" | "mode">;
  ownEmails: string[];
  known: KnownCorrespondents | null;
  groups?: readonly ContactGroupRecipients[] | null;
}): ComposeCheck[] {
  const checks: ComposeCheck[] = [];
  const recipients = draftRecipients(draft, ownEmails);
  const account = draft.account.toLocaleLowerCase();

  const word = authoredText(draft.body).match(ATTACHMENT_WORDS)?.[1];
  if (word && !draft.attachments.some((attachment) => !attachment.inline)) {
    checks.push({ kind: "attachment", word: word.toLocaleLowerCase() });
  }
  if (!draft.subject.trim()) checks.push({ kind: "subject" });

  // Everyone in To and Cc sees every other address there. A large group is
  // usually better in Bcc. Listed before the per-recipient checks, which a
  // large group can make numerous.
  if (groups) {
    const visible = recipients.filter((recipient) => recipient.field !== "bcc").map((recipient) => recipient.email);
    for (const group of overexposedGroups(groups, visible)) {
      checks.push({ kind: "groupExposure", group: group.name, count: group.count, emails: group.emails });
    }
  }

  if (known) {
    const here = known[account] ?? new Map<string, KnownAddress>();
    const anywhere = (email: string) => Object.values(known).some((map) => {
      const history = map.get(email);
      return Boolean(history && (history.sent + history.received > 0 || history.pinned));
    });
    for (const recipient of recipients) {
      if (anywhere(recipient.email)) continue;
      const intended = likelyIntendedAddress(recipient.email, Object.values(known).flatMap((map) => [...map]));
      if (intended) checks.push({ kind: "typo", email: recipient.email, suggestion: intended.email, suggestionName: intended.name });
      // A reply's recipients come from the conversation, so newness is expected there.
      else if (draft.mode === "new" || draft.mode === "forward") checks.push({ kind: "firstContact", email: recipient.email });
    }

    // A new message's account can still change. Suggest another only when
    // every recipient the user has written to was written to from that one
    // account, and none from this one.
    if (draft.mode === "new" && recipients.length > 0) {
      const others = new Set<string>();
      const emails: string[] = [];
      let usedHere = false;
      for (const recipient of recipients) {
        if ((here.get(recipient.email)?.sent ?? 0) > 0) usedHere = true;
        for (const [other, map] of Object.entries(known)) {
          if (other !== account && (map.get(recipient.email)?.sent ?? 0) > 0) {
            others.add(other);
            emails.push(recipient.email);
          }
        }
      }
      if (!usedHere && others.size === 1) checks.push({ kind: "account", account: [...others][0], emails: [...new Set(emails)] });
    }
  }

  // People outside the sender's organization on a mostly internal message.
  // Mixed messages with one colleague are common (introductions, client
  // threads), so this needs at least two colleagues and more of them than
  // outsiders.
  const senderDomain = domainOf(draft.account);
  if (senderDomain && !isPersonalMailDomain(senderDomain)) {
    const outside = recipients.filter((recipient) => domainOf(recipient.email) !== senderDomain);
    const inside = recipients.length - outside.length;
    if (outside.length > 0 && inside >= 2 && inside > outside.length) {
      checks.push({ kind: "outside", domain: senderDomain, emails: outside.map((recipient) => recipient.email) });
    }
  }

  return checks;
}

/**
 * Moves each address in `emails` from To and Cc to the end of Bcc, keeping
 * how each entry was written. Returns only the fields that changed.
 */
export function moveAddressesToBcc(fields: Pick<Draft, "to" | "cc" | "bcc">, emails: readonly string[]): Partial<Pick<Draft, "to" | "cc" | "bcc">> {
  const targets = new Set(emails.map((email) => email.toLocaleLowerCase()));
  const key = (segment: string) => parseAddress(segment).email.trim().toLocaleLowerCase();
  const changes: Partial<Pick<Draft, "to" | "cc" | "bcc">> = {};
  const moved: string[] = [];
  for (const field of ["to", "cc"] as const) {
    const segments = splitAddressList(fields[field]).map((segment) => segment.trim()).filter(Boolean);
    const kept = segments.filter((segment) => !targets.has(key(segment)));
    if (kept.length === segments.length) continue;
    moved.push(...segments.filter((segment) => targets.has(key(segment))));
    changes[field] = kept.join(", ");
  }
  if (!moved.length) return changes;
  const bcc = splitAddressList(fields.bcc).map((segment) => segment.trim()).filter(Boolean);
  const inBcc = new Set(bcc.map(key));
  const added = moved.filter((segment) => {
    if (inBcc.has(key(segment))) return false;
    inBcc.add(key(segment));
    return true;
  });
  if (added.length) changes.bcc = [...bcc, ...added].join(", ");
  return changes;
}

/** Replaces `from` with `to` in an address list, keeping every other entry as written. */
export function replaceAddress(list: string, from: string, to: string): string {
  const target = from.toLocaleLowerCase();
  let changed = false;
  const next = splitAddressList(list).map((segment) => {
    if (parseAddress(segment).email.trim().toLocaleLowerCase() !== target) return segment.trim();
    changed = true;
    return to;
  });
  return changed ? next.join(", ") : list;
}
