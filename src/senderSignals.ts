import type { Message, MessageTrust, SenderRule, SplitInbox, ThreadDetail } from "./domain";
import { parseAddress } from "./emailAddress";

/** Gmail's category labels that mark a conversation as bulk mail. */
export const BULK_CATEGORY_LABELS: readonly string[] = ["CATEGORY_UPDATES", "CATEGORY_PROMOTIONS"];
/** A bulk sender's conversations Recent emails can add. */
export const SENDER_HISTORY_LIMIT = 20;
/** Mismatched links listed in the Trust section; more are counted in the last one. */
export const TRUST_MISMATCH_LIMIT = 3;

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

export type LinkMismatch = { shown: string; opens: string };
export type LinkAnalysis = {
  /** Sites the message's web links open, most linked first. */
  sites: string[];
  /** Links whose visible text names one site while the link opens another. */
  mismatches: LinkMismatch[];
  /** Remote images sized or styled to be invisible, the usual open trackers. */
  trackingImages: number;
};

/** Link text that is itself an address or URL: a host, optionally with a path. */
const SHOWN_ADDRESS = /^(?:https?:\/\/)?((?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,})(?::\d+)?(?:[/?#]\S*)?$/i;
const HIDDEN_STYLE = /(?:^|;)\s*(?:display\s*:\s*none|visibility\s*:\s*hidden|(?:width|height)\s*:\s*[01](?:px)?\s*(?:;|$))/i;

function webHost(value: string | null): string | null {
  if (!value) return null;
  try {
    const url = new URL(value.trim());
    return url.protocol === "https:" || url.protocol === "http:" ? url.hostname : null;
  } catch {
    return null;
  }
}

function tiny(value: string | null): boolean {
  if (value === null) return false;
  const size = Number.parseFloat(value);
  return Number.isFinite(size) && size <= 1;
}

/**
 * Reads a message body's links and images without rendering it. The body is
 * parsed into an inert document: nothing loads, runs, or leaves the device,
 * so remote images stay behind the reader's own proxy and blocking.
 */
export function analyzeLinks(html: string): LinkAnalysis {
  if (!html.trim()) return { sites: [], mismatches: [], trackingImages: 0 };
  const document = new DOMParser().parseFromString(html, "text/html");
  const counts = new Map<string, number>();
  const mismatches: LinkMismatch[] = [];
  for (const anchor of document.querySelectorAll("a[href]")) {
    const host = webHost(anchor.getAttribute("href"));
    if (!host) continue;
    const site = siteOf(host);
    counts.set(site, (counts.get(site) ?? 0) + 1);
    const text = (anchor.textContent ?? "").trim();
    const shown = text.includes("@") ? null : SHOWN_ADDRESS.exec(text)?.[1];
    if (shown && siteOf(shown) !== site && !mismatches.some((item) => item.shown === siteOf(shown) && item.opens === site)) {
      mismatches.push({ shown: siteOf(shown), opens: site });
    }
  }
  let trackingImages = 0;
  for (const image of document.querySelectorAll("img[src]")) {
    if (!webHost(image.getAttribute("src"))) continue;
    const sized = tiny(image.getAttribute("width")) && tiny(image.getAttribute("height"));
    if (sized || HIDDEN_STYLE.test(image.getAttribute("style") ?? "")) trackingImages += 1;
  }
  const sites = [...counts].sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0])).map(([site]) => site);
  return { sites, mismatches, trackingImages };
}

export type TrustFinding = {
  key: string;
  /** Warnings come first and bring the section into any conversation. */
  tone: "warning" | "verified" | "info";
  kind: "authentication" | "replyTo" | "mismatch" | "sites" | "tracking";
  title: string;
  detail: string;
};

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

/**
 * What the Trust section lists, warnings first: the receiving server's verdict
 * on the sender, a Reply-To at another organization, links whose text names a
 * different site than they open, the sites links open, and tracking images.
 * `trust` is null when the message's headers are not stored.
 */
export function trustFindings(trust: MessageTrust | null, links: LinkAnalysis): TrustFinding[] {
  const findings: TrustFinding[] = [];
  if (trust) findings.push(authenticationFinding(trust));
  if (trust?.replyTo) {
    findings.push({ key: "reply-to", tone: "warning", kind: "replyTo", title: "Replies go elsewhere", detail: `Reply-To is ${trust.replyTo}` });
  }
  const listed = links.mismatches.slice(0, TRUST_MISMATCH_LIMIT);
  listed.forEach((item, index) => {
    const extra = index === listed.length - 1 ? links.mismatches.length - listed.length : 0;
    findings.push({
      key: `mismatch:${item.shown}:${item.opens}`,
      tone: "warning",
      kind: "mismatch",
      title: "Link text doesn't match",
      detail: `Shows ${item.shown}, opens ${item.opens}${extra > 0 ? ` (and ${extra} more)` : ""}`,
    });
  });
  if (links.sites.length > 0) {
    findings.push({
      key: "sites",
      tone: "info",
      kind: "sites",
      title: links.sites.length === 1 ? "Links open 1 site" : `Links open ${links.sites.length} sites`,
      detail: links.sites.join(", "),
    });
  }
  if (links.trackingImages > 0) {
    findings.push({
      key: "tracking",
      tone: "info",
      kind: "tracking",
      title: links.trackingImages === 1 ? "1 tracking image" : `${links.trackingImages} tracking images`,
      detail: "Tells the sender you opened this email if remote images load",
    });
  }
  const rank = { warning: 0, verified: 1, info: 2 } as const;
  return findings.map((finding, index) => ({ finding, index }))
    .sort((left, right) => rank[left.finding.tone] - rank[right.finding.tone] || left.index - right.index)
    .map(({ finding }) => finding);
}
