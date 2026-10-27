import { attachmentRisk } from "./attachmentRisk";
import type { Message, MessageAttachment, MessageTrust, SenderDeliveryPattern, SenderFamiliarity, SenderRule, SplitInbox, ThreadDetail } from "./domain";
import { parseAddress } from "./emailAddress";
import { disguisedWords, knownSites, lookalikeOf, mixesScripts, spelledOutSite, unicodeHost, type KnownSites } from "./lookalike";
import { foldQuotedHistoryHtml, type PriorThreadText } from "./quotedHistory";
import { sanitizeMessageHtml } from "./SafeMessage";

/** Gmail's category labels that mark a conversation as bulk mail. */
export const BULK_CATEGORY_LABELS: readonly string[] = ["CATEGORY_UPDATES", "CATEGORY_PROMOTIONS"];
/** A bulk sender's conversations Recent emails can add. */
export const SENDER_HISTORY_LIMIT = 20;
/** Link warnings listed in the Trust section; more are counted in the last one. */
export const TRUST_MISMATCH_LIMIT = 3;
/** Risky attachments listed in the Trust section. */
export const TRUST_ATTACHMENT_LIMIT = 3;
/** Distinct links read from one message. */
export const TRUST_LINK_LIMIT = 500;
/** Disguised subject words the Trust section names before counting the rest. */
export const TRUST_SUBJECT_WORD_LIMIT = 3;

/** The newest message in a conversation that someone else sent. */
export function latestInbound(detail: ThreadDetail, ownEmails: Set<string>): Message | null {
  for (const message of [...detail.messages].reverse()) {
    if (!ownEmails.has(parseAddress(message.sender).email.toLocaleLowerCase())) return message;
  }
  return null;
}

/**
 * The newest inbound message when the conversation is bulk mail: it carries a
 * List-Unsubscribe option, or the conversation is in the Updates or
 * Promotions category. Decided only from those two signals, never from who
 * the sender is.
 */
export function bulkMessage(detail: ThreadDetail, ownEmails: Set<string>): Message | null {
  const message = latestInbound(detail, ownEmails);
  if (!message) return null;
  const listed = Boolean(message.unsubscribe?.methods.length);
  const categorized = detail.thread.labels.some((label) => BULK_CATEGORY_LABELS.includes(label));
  return listed || categorized ? message : null;
}

/** The account's split inbox that already takes this sender's mail, if any. */
export function matchingSplit(splits: SplitInbox[], accountId: string, email: string): SplitInbox | null {
  const address = email.toLocaleLowerCase();
  const domain = address.split("@")[1] ?? "";
  return splits.find((split) => split.accountId === accountId && (
    (split.matchKind === "pattern" && address.includes(split.matchValue))
    || (split.matchKind === "domain" && domain === split.matchValue)
  )) ?? null;
}

/** The account's mute rule for this sender: their exact address, or their exact domain. */
export function matchingRule(rules: SenderRule[], accountId: string, email: string): SenderRule | null {
  const address = email.toLocaleLowerCase();
  const domain = address.split("@")[1] ?? "";
  const account = accountId.toLocaleLowerCase();
  return rules.find((rule) => rule.accountId.toLocaleLowerCase() === account && (
    (rule.matchKind === "address" && rule.matchValue === address)
    || (rule.matchKind === "domain" && rule.matchValue === domain)
  )) ?? null;
}

/** Two-letter country domains that register names one level down (example.co.uk). */
const SECOND_LEVEL = new Set(["ac", "co", "com", "edu", "go", "gov", "ne", "net", "or", "org"]);

/**
 * The site a host belongs to, so `click.example.com` and `www.example.com`
 * compare equal: the last two labels, or three under a country domain that
 * registers one level down. IP addresses stay whole.
 */
export function siteOf(host: string): string {
  const clean = host.toLocaleLowerCase().replace(/\.$/, "").replace(/^www\./, "");
  if (/^[\d.]+$/.test(clean) || clean.includes(":")) return clean;
  const labels = clean.split(".");
  if (labels.length <= 2) return clean;
  const last = labels[labels.length - 1];
  const second = labels[labels.length - 2];
  return labels.slice(last.length === 2 && SECOND_LEVEL.has(second) ? -3 : -2).join(".");
}

/** One link in the message, as the reader shows it: where it goes and its visible text. */
export type TrustLink = { href: string; text: string };
export type LinkMismatch = { shown: string; opens: string; link: TrustLink };
/** A web link read from the shown part of a message. */
export type LinkFact = {
  link: TrustLink;
  /** The host the link opens, as the URL parser gives it (Punycode for international names). */
  host: string;
  site: string;
  /** Text before an `@` in the address, which browsers skip: `https://bank.example@other.example`. */
  userinfo: string | null;
};
export type LinkAnalysis = {
  /** Sites the shown part's web links open, most linked first. */
  sites: string[];
  /** Links whose visible text names one site while the link opens another. */
  mismatches: LinkMismatch[];
  /** Distinct web links in the shown part, in document order. */
  links: LinkFact[];
  /** Remote images sized or styled to be invisible, the usual open trackers. */
  trackingImages: number;
};

/** Link text that is itself an address or URL: a host, optionally with a path. */
const SHOWN_ADDRESS = /^(?:https?:\/\/)?((?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,})(?::\d+)?(?:[/?#]\S*)?$/i;
const HIDDEN_STYLE = /(?:^|;)\s*(?:display\s*:\s*none|visibility\s*:\s*hidden|(?:width|height)\s*:\s*[01](?:px)?\s*(?:;|$))/i;

function webUrl(value: string | null): URL | null {
  if (!value) return null;
  try {
    const url = new URL(value.trim());
    return url.protocol === "https:" || url.protocol === "http:" ? url : null;
  } catch {
    return null;
  }
}

function decodedUserinfo(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function tiny(value: string | null): boolean {
  if (value === null) return false;
  const size = Number.parseFloat(value);
  return Number.isFinite(size) && size <= 1;
}

/**
 * The part of a message the reader shows before its folded quoted history:
 * the same sanitize-then-fold cut the message view makes, or the whole
 * sanitized body when nothing folds. Sanitizing parks every remote image
 * behind the blocked-source marker, so building it loads nothing.
 */
function shownHtml(html: string, prior?: PriorThreadText): string {
  const sanitized = sanitizeMessageHtml(html);
  return foldQuotedHistoryHtml(sanitized, prior)?.visible ?? sanitized;
}

/**
 * Reads a message body's links and images without rendering it. Links are
 * read only from the part the reader shows before folded quoted history, so
 * a link someone else's mail system rewrote in an earlier, quoted email never
 * warns about this one; `prior` is the conversation text before the message,
 * as the message view folds with it. Tracking images are counted across the
 * whole body, since any of them can load. Bodies are parsed into inert
 * documents: nothing loads, runs, or leaves the device, so remote images stay
 * behind the reader's own proxy and blocking.
 */
export function analyzeLinks(html: string, prior?: PriorThreadText): LinkAnalysis {
  if (!html.trim()) return { sites: [], mismatches: [], links: [], trackingImages: 0 };
  const parser = new DOMParser();
  const document = parser.parseFromString(html, "text/html");
  const shown = parser.parseFromString(shownHtml(html, prior), "text/html");
  const counts = new Map<string, number>();
  const mismatches: LinkMismatch[] = [];
  const links: LinkFact[] = [];
  const seen = new Set<string>();
  for (const anchor of shown.querySelectorAll("a[href]")) {
    const href = anchor.getAttribute("href") ?? "";
    const url = webUrl(href);
    if (!url) continue;
    const host = url.hostname.replace(/^\[|\]$/g, "");
    const site = siteOf(host);
    counts.set(site, (counts.get(site) ?? 0) + 1);
    const text = (anchor.textContent ?? "").trim();
    const link = { href, text };
    const key = `${href}\n${text}`;
    if (!seen.has(key) && links.length < TRUST_LINK_LIMIT) {
      seen.add(key);
      links.push({ link, host, site, userinfo: url.username ? decodedUserinfo(url.username) : null });
    }
    const shownHost = text.includes("@") ? null : SHOWN_ADDRESS.exec(text)?.[1];
    if (shownHost && siteOf(shownHost) !== site && !mismatches.some((item) => item.shown === siteOf(shownHost) && item.opens === site)) {
      mismatches.push({ shown: siteOf(shownHost), opens: site, link });
    }
  }
  let trackingImages = 0;
  for (const image of document.querySelectorAll("img[src]")) {
    if (!webUrl(image.getAttribute("src"))) continue;
    const sized = tiny(image.getAttribute("width")) && tiny(image.getAttribute("height"));
    if (sized || HIDDEN_STYLE.test(image.getAttribute("style") ?? "")) trackingImages += 1;
  }
  const sites = [...counts].sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0])).map(([site]) => site);
  return { sites, mismatches, links, trackingImages };
}

export type TrustFinding = {
  key: string;
  /** Warnings come first and bring the section into any conversation. */
  tone: "warning" | "verified" | "info";
  kind: "authentication" | "replyTo" | "mismatch" | "sites" | "tracking" | "impersonation" | "lookalike" | "subject" | "link" | "attachment" | "history" | "delivery";
  title: string;
  detail: string;
  /** Addresses to compare when a familiar display name uses a different address. */
  addressComparison?: { name: string; sending: string; previous: string[] };
  /** The link a link finding is about, so the reader can show it. */
  link?: TrustLink;
  /**
   * A warning strong enough to head the message itself: a borrowed name, a
   * look-alike sender or link domain, or a risky attachment.
   */
  strong?: boolean;
};

/**
 * What the Trust section shows. `summary` heads the section: "careful" when
 * anything warns, "known" for someone the user writes to or saved whose mail
 * passed DMARC (or, with no DMARC result, SPF or DKIM for its own domain),
 * otherwise none. `rows` are the warnings and the sender's
 * history; `details` holds the rest, folded under Details.
 */
export type TrustReport = {
  summary: "careful" | "known" | null;
  rows: TrustFinding[];
  details: TrustFinding[];
  feedback?: {
    requester: string;
    replyTo: string | null;
    linkOrigins: string[];
    patterns: SenderDeliveryPattern[];
    matching: SenderDeliveryPattern | null;
    /** Keep the section available for mediated mail and remembered relationships. */
    relevant: boolean;
  };
};

export type TrustInput = {
  messageId?: string;
  /** Null when the message's headers are not stored. */
  trust: MessageTrust | null;
  links: LinkAnalysis;
  /** Null when the history could not be read; checks that need it are skipped. */
  familiarity: SenderFamiliarity | null;
  /** The From header as stored. */
  sender: string;
  /** The conversation's subject line. */
  subject: string;
  attachments: readonly MessageAttachment[];
  bulk: boolean;
};

/** Exact web origins, including scheme and port; never retain document URLs or tokens. */
export function deliveryLinkOrigins(links: LinkAnalysis): string[] {
  return [...new Set(links.links.map((fact) => new URL(fact.link.href).origin))].sort();
}

const METHOD_NAMES = { dmarc: "DMARC", dkim: "DKIM", spf: "SPF" } as const;
/** Results that are a check's real conclusion against the sender, not a missing or neutral one. */
const FAILING = new Set(["fail", "softfail", "permerror"]);

function joinNames(names: string[]): string {
  if (names.length <= 1) return names.join("");
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  return `${names.slice(0, -1).join(", ")}, and ${names.at(-1)}`;
}

function sameOrganization(left: string, right: string): boolean {
  return left === right || left.endsWith(`.${right}`) || right.endsWith(`.${left}`);
}

function authenticationFinding(trust: MessageTrust): TrustFinding {
  const domain = trust.fromDomain || "the sender";
  const verdicts = trust.authentication;
  const base = { key: "authentication", kind: "authentication" as const };
  if (!verdicts) {
    return { ...base, tone: "info", title: "No sender checks", detail: "Your mail server reported no SPF, DKIM, or DMARC result" };
  }
  const passed = (["dmarc", "dkim", "spf"] as const).filter((method) => verdicts[method] === "pass").map((method) => METHOD_NAMES[method]);
  if (verdicts.dmarc === "pass") {
    return { ...base, tone: "verified", title: "Sender verified", detail: `${domain} passed ${joinNames(passed)}` };
  }
  // Without a DMARC result, a pass for the From domain's own organization is
  // what DMARC would have checked (relaxed alignment), so it verifies too.
  const aligned = ([["dkim", verdicts.dkimDomains], ["spf", verdicts.spfDomains]] as const)
    .filter(([, domains]) => trust.fromDomain && domains.some((signer) => sameOrganization(signer, trust.fromDomain)))
    .map(([method]) => METHOD_NAMES[method]);
  if (!verdicts.dmarc && aligned.length > 0) {
    return { ...base, tone: "verified", title: "Sender verified", detail: `${domain} passed ${joinNames(aligned)} for its own domain` };
  }
  if (verdicts.dmarc && FAILING.has(verdicts.dmarc)) {
    return { ...base, tone: "warning", title: "Sender not verified", detail: `${domain} failed DMARC` };
  }
  if (passed.length > 0) {
    const signers = verdicts.dkimDomains.filter((signer) => !sameOrganization(signer, trust.fromDomain));
    const signedBy = verdicts.dkim === "pass" && signers.length > 0 ? `; signed by ${signers.join(", ")}` : "";
    return { ...base, tone: "info", title: "Partly verified", detail: `${joinNames(passed)} passed, no DMARC result${signedBy}` };
  }
  if ((verdicts.dkim && FAILING.has(verdicts.dkim)) || (verdicts.spf && FAILING.has(verdicts.spf))) {
    return { ...base, tone: "warning", title: "Sender not verified", detail: `Neither SPF nor DKIM passed for ${domain}` };
  }
  return { ...base, tone: "info", title: "No sender checks", detail: "Your mail server reported no passing or failing result" };
}

/** A host in its ASCII (Punycode) form, as the URL parser writes it. */
function asciiHost(host: string): string {
  try {
    return new URL(`http://${host}`).hostname;
  } catch {
    return host;
  }
}

/** A host as people read it, with its real ASCII form when the two differ. */
function describeHost(host: string): string {
  const shown = unicodeHost(asciiHost(host));
  const ascii = asciiHost(host);
  return shown === ascii ? shown : `${shown} (${ascii})`;
}

function registrableName(site: string): string {
  return unicodeHost(site).split(".")[0] ?? "";
}

/** An address written into a display name: `"support@bank.example" <…>`. */
const NAME_ADDRESS = /[\w.+-]+@((?:[a-z0-9-]+\.)+[a-z]{2,})/i;
/**
 * A domain written into a display name, such as "Shop.example" or "news.example.org".
 * The suffix must be lowercase and two or three letters, so initials and
 * abbreviations ("J.R. Doe", "Mr.Smith") are not read as domains.
 */
const NAME_DOMAIN = /(?:^|[\s(<[,"'])((?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)*[a-z0-9][a-z0-9-]*[a-z0-9]\.[a-z]{2,3})(?=$|[\s)>\],:;"'])/i;
const LOWERCASE_SUFFIX = /\.[a-z]{2,3}$/;

/** The address or domain a display name claims, when it names another organization than the sender's. */
function claimedInName(name: string, email: string): string | null {
  const domain = email.split("@")[1] ?? "";
  if (!domain) return null;
  const senderName = registrableName(siteOf(domain));
  const address = NAME_ADDRESS.exec(name);
  if (address) {
    return registrableName(siteOf(address[1])) === senderName ? null : address[0];
  }
  const shown = NAME_DOMAIN.exec(name)?.[1];
  if (!shown || !LOWERCASE_SUFFIX.test(shown)) return null;
  return registrableName(siteOf(shown.toLocaleLowerCase())) === senderName ? null : shown;
}

/** How a subject disguises its words, words with the same trick together: “REVlEW” and “REClEVE” use a lowercase l in place of I. */
function subjectDisguises(subject: string): string | null {
  const found = disguisedWords(subject);
  if (found.length === 0) return null;
  const listed = found.slice(0, TRUST_SUBJECT_WORD_LIMIT);
  const tricks = new Map<string, string[]>();
  for (const { word, trick } of listed) tricks.set(trick, [...(tricks.get(trick) ?? []), `“${word}”`]);
  const extra = found.length - listed.length;
  const described = [...tricks].map(([trick, words]) => `${joinNames(words)} ${words.length === 1 ? "uses" : "use"} ${trick}`).join("; ");
  return extra > 0 ? `${described} (and ${extra} more)` : described;
}

function monthYear(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString(undefined, { month: "short", year: "numeric" });
}

function times(count: number): string {
  return count === 1 ? "once" : count === 2 ? "twice" : `${count} times`;
}

/** At most one warning per link, the most specific trick first. */
function linkTrick(fact: LinkFact, known: KnownSites | null): Omit<TrustFinding, "key" | "tone" | "kind" | "link"> & { site: string; kind: string } | null {
  if (fact.userinfo) {
    return { kind: "userinfo", site: fact.site, title: "Link hides where it goes", detail: `Shows ${fact.userinfo}, opens ${describeHost(fact.site)}` };
  }
  const imitated = known ? lookalikeOf(fact.site, known) ?? spelledOutSite(fact.host, fact.site, known) : null;
  if (imitated) {
    return { kind: "lookalike", site: fact.site, title: `Link looks like ${unicodeHost(imitated)}`, detail: `Opens ${describeHost(fact.host)}` };
  }
  if (unicodeHost(fact.host).split(".").some(mixesScripts)) {
    return { kind: "script", site: fact.site, title: "Link uses look-alike letters", detail: `Opens ${describeHost(fact.host)}` };
  }
  if (/^[\d.]+$/.test(fact.host) || fact.host.includes(":")) {
    return { kind: "address", site: fact.site, title: "Link opens a numeric address", detail: `Opens ${fact.host}` };
  }
  return null;
}

/**
 * What the Trust section shows, warnings first: the receiving server's
 * verdict on the sender; a display name that belongs to someone the user
 * knows at another address, or that spells out another organization; a
 * From domain or link that imitates one the user corresponds with; subject
 * words written with look-alike characters; links
 * whose text or address hides where they go; risky attachments; and a
 * Reply-To at another organization. The sender's history with the user
 * follows. The sites links open and tracking images sit in the details.
 */
export function trustReport({ messageId, trust, links, familiarity, sender, subject, attachments, bulk }: TrustInput): TrustReport {
  const from = parseAddress(sender);
  const email = from.email.toLocaleLowerCase();
  const domain = email.split("@")[1] ?? "";
  const fromSite = domain ? siteOf(domain) : "";
  const known = familiarity ? knownSites(familiarity.knownDomains.map(siteOf)) : null;
  const acquainted = Boolean(familiarity && (familiarity.sentCount > 0 || familiarity.contactName));
  const warnings: TrustFinding[] = [];
  const details: TrustFinding[] = [];
  const warn = (finding: Omit<TrustFinding, "tone">) => warnings.push({ ...finding, tone: "warning" });

  const authentication = trust ? authenticationFinding(trust) : null;
  if (authentication?.tone === "warning") warnings.push(authentication);

  // Exact identities and origins prevent a confirmation becoming a blanket
  // exemption for a shared service or organization. Missing feedback reads
  // and truncated link analysis never authorize a quieter assessment.
  const patterns = familiarity?.deliveryPatterns;
  const linkOrigins = deliveryLinkOrigins(links);
  const requester = trust?.replyTo?.toLocaleLowerCase() || email;
  const matching = patterns?.find((pattern) => pattern.senderEmail === email
    && pattern.replyTo === (trust?.replyTo?.toLocaleLowerCase() ?? null)
    && pattern.linkOrigins.length === linkOrigins.length
    && pattern.linkOrigins.every((origin, index) => origin === linkOrigins[index])) ?? null;
  const feedback = trust && patterns && email.includes("@") && domain === trust.fromDomain && links.links.length < TRUST_LINK_LIMIT
    ? { requester, replyTo: trust.replyTo, linkOrigins, patterns, matching, relevant: Boolean(trust.replyTo || patterns.length) }
    : undefined;
  const recognizedDelivery = Boolean(feedback && matching?.authenticated && trust?.authentication?.dmarc === "pass");
  const confirmedMessage = Boolean(feedback && messageId && matching?.sourceMessageId === messageId);
  if (feedback && !matching && patterns?.some((pattern) => pattern.authenticated)) {
    warn({ key: "delivery-changed", kind: "delivery", title: "Different delivery pattern", detail: `You've confirmed other requests for ${requester}; this sending address or its link destinations are different` });
  }

  // Confirming this exact delivery answers the borrowed-name question, but
  // only for authenticated mail or the message the user confirmed.
  if (familiarity && !acquainted && familiarity.nameMatches.length > 0 && from.name && !recognizedDelivery && !confirmedMessage) {
    const others = familiarity.nameMatches.length - 1;
    warn({
      key: "impersonation",
      kind: "impersonation",
      title: "Familiar name, different email address",
      addressComparison: { name: from.name, sending: email, previous: familiarity.nameMatches },
      strong: true,
      detail: `${from.name} · Sending address: ${email} · Previously seen: ${familiarity.nameMatches[0]}${others > 0 ? ` and ${others} more` : ""}`,
    });
  }
  const claimed = from.name ? claimedInName(from.name, email) : null;
  if (claimed) {
    warn({ key: "claimed-name", kind: "impersonation", title: `Name shows ${claimed}`, detail: `Sent from ${email}`, strong: true });
  }
  const imitated = known && fromSite ? lookalikeOf(fromSite, known) : null;
  if (imitated) {
    warn({ key: "lookalike", kind: "lookalike", title: `Address looks like ${unicodeHost(imitated)}`, detail: `Sent from ${describeHost(domain)}`, strong: true });
  } else if (domain && unicodeHost(asciiHost(domain)).split(".").some(mixesScripts)) {
    warn({ key: "lookalike", kind: "lookalike", title: "Address uses look-alike letters", detail: `Sent from ${describeHost(domain)}`, strong: true });
  }

  const disguised = subjectDisguises(subject);
  if (disguised) warn({ key: "subject", kind: "subject", title: "Subject uses look-alike letters", detail: disguised });

  if (trust?.replyTo) {
    const replyDomain = trust.replyTo.split("@")[1]?.toLocaleLowerCase() ?? "";
    const familiarReply = Boolean(known && replyDomain && known.sites.has(siteOf(replyDomain)));
    const finding: TrustFinding = {
      key: "reply-to",
      tone: familiarReply || recognizedDelivery ? "info" : "warning",
      kind: "replyTo",
      title: "Replies go elsewhere",
      detail: recognizedDelivery ? `Reply-To is ${trust.replyTo}, matching a request you confirmed` : familiarReply ? `Reply-To is ${trust.replyTo}, a domain you write to` : `Reply-To is ${trust.replyTo}`,
    };
    if (familiarReply || recognizedDelivery) details.push(finding);
    else warnings.push(finding);
  }

  const linkWarnings: TrustFinding[] = links.mismatches.map((item) => ({
    key: `mismatch:${item.shown}:${item.opens}`,
    tone: "warning",
    kind: "mismatch",
    title: "Link text doesn't match",
    detail: `Shows ${item.shown}, opens ${item.opens}`,
    link: item.link,
  }));
  const mismatched = new Set(links.mismatches.map((item) => item.link.href));
  for (const fact of links.links) {
    if (mismatched.has(fact.link.href)) continue;
    const trick = linkTrick(fact, known);
    if (!trick) continue;
    const key = `link:${trick.kind}:${trick.site}`;
    if (linkWarnings.some((finding) => finding.key === key)) continue;
    const strong = trick.kind === "lookalike" || trick.kind === "script";
    linkWarnings.push({ key, tone: "warning", kind: "link", title: trick.title, detail: trick.detail, link: fact.link, ...(strong ? { strong } : {}) });
  }
  const listed = linkWarnings.slice(0, TRUST_MISMATCH_LIMIT);
  const extra = linkWarnings.length - listed.length;
  if (extra > 0) {
    const last = listed[listed.length - 1];
    listed[listed.length - 1] = { ...last, detail: `${last.detail} (and ${extra} more)` };
  }
  warnings.push(...listed);

  const risky = attachments
    .filter((attachment) => !attachment.inline)
    .map((attachment) => ({ attachment, risk: attachmentRisk(attachment.filename, attachment.mimeType) }))
    // Office files and disk images are everyday attachments between people
    // who know each other; programs and disguised names never are.
    .filter(({ risk }) => risk && (!acquainted || risk.kind === "program" || risk.kind === "disguised"));
  for (const { attachment, risk } of risky.slice(0, TRUST_ATTACHMENT_LIMIT)) {
    warn({ key: `attachment:${attachment.id}`, kind: "attachment", title: attachment.filename, detail: risk?.reason ?? "", strong: true });
  }

  const history: TrustFinding[] = [];
  if (recognizedDelivery || confirmedMessage) {
    history.push({ key: "delivery-confirmed", tone: "info", kind: "delivery", title: confirmedMessage ? "You confirmed this email is legitimate" : "Matches a request you confirmed", detail: `${requester} via ${email}` });
  }
  if (familiarity) {
    const who = familiarity.contactName || from.name || email;
    if (familiarity.sentCount > 0) {
      const since = familiarity.firstSentAt ? monthYear(familiarity.firstSentAt) : "";
      history.push({
        key: "history",
        tone: "verified",
        kind: "history",
        title: `You've written to ${who.split(/\s+/)[0] || email} ${times(familiarity.sentCount)}`,
        detail: since ? `Since ${since}` : email,
      });
    } else if (familiarity.contactName) {
      history.push({ key: "history", tone: "verified", kind: "history", title: "Saved in your contacts", detail: `As ${familiarity.contactName}` });
    } else if (!bulk && !familiarity.receivedBefore && email && !recognizedDelivery && !confirmedMessage) {
      history.push({ key: "history", tone: "info", kind: "history", title: "First email from this address", detail: email });
    }
  }

  if (authentication && authentication.tone !== "warning") details.unshift(authentication);
  if (links.sites.length > 0) {
    details.push({
      key: "sites",
      tone: "info",
      kind: "sites",
      title: links.sites.length === 1 ? "Links open 1 site" : `Links open ${links.sites.length} sites`,
      detail: links.sites.join(", "),
    });
  }
  if (links.trackingImages > 0) {
    details.push({
      key: "tracking",
      tone: "info",
      kind: "tracking",
      title: links.trackingImages === 1 ? "1 tracking image" : `${links.trackingImages} tracking images`,
      detail: "Tells the sender you opened this email if remote images load",
    });
  }

  const rows = [...warnings, ...history];
  // With nothing to warn about or recall, the sender check is the headline.
  if (rows.length === 0 && details[0]?.kind === "authentication") rows.push(details.shift()!);
  const summary = warnings.length > 0
    ? "careful"
    : acquainted && authentication?.tone === "verified" ? "known" : null;
  return { summary, rows, details, ...(feedback ? { feedback } : {}) };
}

/**
 * The reader header's one line about a report that warns: the first warning,
 * with a count of the rest. Null when nothing warns.
 */
export function trustHeadline(report: TrustReport): string | null {
  const warnings = report.rows.filter((finding) => finding.tone === "warning");
  if (warnings.length === 0) return null;
  return warnings.length === 1 ? warnings[0].title : `${warnings[0].title} · ${warnings.length - 1} more`;
}
