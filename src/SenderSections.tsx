import { useEffect, useId, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Eye, Globe, Link2, Mail, Paperclip, Reply, Shield, ShieldCheck, TriangleAlert, UserCheck, UserRound } from "lucide-react";
import type { Message, MessageTrust, SenderFamiliarity, SenderHistory, SenderRule, SplitInbox } from "./domain";
import { parseAddress } from "./emailAddress";
import { mailClient } from "./data/client";
import { ContextRow, ContextSection } from "./ContextSections";
import { ICON_SIZE } from "./iconSizes";
import type { PriorThreadText } from "./quotedHistory";
import { logBackgroundFailure } from "./errors";
import { analyzeLinks, matchingRule, matchingSplit, SENDER_HISTORY_LIMIT, trustReport, type TrustFinding, type TrustLink, type TrustReport } from "./senderSignals";

/** The bulk sender a Sender section is about. */
export type SenderTarget = { accountId: string; email: string; name: string };

/** The one-click decisions the Sender section offers, supplied by the app. */
export type SenderActions = {
  /** Split inboxes across accounts; one that already takes the sender's mail replaces New Split. */
  splits: SplitInbox[];
  /** Opens the unsubscribe confirmation; absent when the message offers no unsubscribe option. */
  onUnsubscribe?(): void;
  /** Archives the conversations, with Undo. */
  onArchiveThreads(ids: string[]): void;
  /** Opens the New Split dialog for the sender. */
  onNewSplit(target: SenderTarget): void;
  /** Muted senders across accounts; one that covers the sender replaces Mute with Unmute. */
  rules: SenderRule[];
  /** Opens the Mute dialog, offering to archive the conversations still in the Inbox. */
  onMute(target: SenderTarget, inboxThreadIds: string[]): void;
  onUnmute(rule: SenderRule): void;
};

/**
 * What the user has received from a bulk sender in this account, reloaded
 * when the open conversation changes. `settled` turns true once the read
 * finishes, so the panel can wait for it rather than reorder rows; a failed
 * read leaves `history` null.
 */
export function useSenderHistory(target: SenderTarget | null, threadId: string) {
  const accountId = target?.accountId ?? "";
  const email = target?.email ?? "";
  const [state, setState] = useState<{ key: string; history: SenderHistory | null } | null>(null);
  const key = `${accountId}\n${email}\n${threadId}`;
  useEffect(() => {
    if (!email) return;
    let active = true;
    mailClient.senderHistory(accountId, email, SENDER_HISTORY_LIMIT)
      .then((loaded) => { if (active) setState({ key, history: loaded }); })
      .catch((reason: unknown) => {
        // The history is supplementary; a failed read leaves its rows out.
        if (active) setState({ key, history: null });
        logBackgroundFailure("Sender history")(reason);
      });
    return () => { active = false; };
  }, [accountId, email, key]);
  const settled = Boolean(email) && state?.key === key;
  return { history: settled ? state.history : null, settled };
}

/**
 * What to do about a bulk sender, as quiet links under Recent emails:
 * unsubscribe, archive what is still in the Inbox, mute the sender so sync
 * archives their new mail, or give the sender a split. A mute or split that
 * already covers the sender replaces its link with the current state.
 */
export function SenderDecisions({ target, history, actions }: {
  target: SenderTarget;
  history: SenderHistory | null;
  actions: SenderActions;
}) {
  const { accountId, email } = target;
  // Hides Archive once used; the panel remounts this for the next conversation.
  const [archived, setArchived] = useState(false);
  const split = matchingSplit(actions.splits, accountId, email);
  const rule = matchingRule(actions.rules, accountId, email);
  const inbox = archived || !history ? [] : history.inboxThreadIds;
  // Archiving one conversation is the reader's own Archive; this is for the rest.
  const canArchive = inbox.length > 1;
  return (
    <div className="context-sender-actions">
      {actions.onUnsubscribe ? <button type="button" className="btn-link" onClick={actions.onUnsubscribe}>Unsubscribe</button> : null}
      {canArchive ? (
        <button type="button" className="btn-link" onClick={() => { setArchived(true); actions.onArchiveThreads(inbox); }}>
          Archive {inbox.length} in Inbox
        </button>
      ) : null}
      {rule
        ? <button type="button" className="btn-link" title={rule.matchKind === "domain" ? `Muted, with everyone at ${rule.matchValue}` : "Muted"} onClick={() => actions.onUnmute(rule)}>Unmute</button>
        : <button type="button" className="btn-link" onClick={() => actions.onMute(target, inbox)}>Mute…</button>}
      {split
        ? <span className="context-sender-state">Sorted into {split.name}</span>
        : <button type="button" className="btn-link" onClick={() => actions.onNewSplit(target)}>New Split…</button>}
    </div>
  );
}

const FINDING_ICONS: Record<TrustFinding["kind"], typeof Shield> = {
  authentication: Shield,
  replyTo: Reply,
  mismatch: Link2,
  link: Link2,
  sites: Link2,
  tracking: Eye,
  impersonation: UserRound,
  lookalike: Globe,
  subject: Mail,
  attachment: Paperclip,
  history: UserRound,
};

function findingIcon(finding: TrustFinding) {
  if (finding.tone === "warning") return <TriangleAlert size={ICON_SIZE.sm} />;
  const Icon = finding.tone !== "verified" ? FINDING_ICONS[finding.kind]
    : finding.kind === "history" ? UserCheck : ShieldCheck;
  return <Icon size={ICON_SIZE.sm} />;
}

/** One Trust finding as a context row; link findings offer Show link. */
export function trustRow(finding: TrustFinding, onShowLink?: (link: TrustLink) => void) {
  const link = finding.link;
  return (
    <ContextRow
      key={finding.key}
      className={finding.tone === "warning" ? "context-row-warning" : finding.tone === "verified" ? "context-row-verified" : undefined}
      icon={findingIcon(finding)}
      title={finding.title}
      detail={finding.detail}
      trailing={link && onShowLink ? <button type="button" className="btn-link" onClick={() => onShowLink(link)}>Show link</button> : undefined}
    />
  );
}

/** The one-line summary of what Details holds, such as "Sender checks · 3 sites · 1 tracking image". */
function detailsSummary(details: TrustFinding[]): string {
  return details.map((finding) => {
    if (finding.kind === "authentication") return "Sender checks";
    if (finding.kind === "replyTo") return "Reply-To";
    if (finding.kind === "sites") return finding.title.replace(/^Links open /, "");
    return finding.title;
  }).join(" · ");
}

/** The folded Details under a trust report: sender checks, sites, and tracking. */
export function TrustDetails({ details }: { details: TrustFinding[] }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  if (details.length === 0) return null;
  return (
    <div className="context-trust-details">
      <button type="button" className="context-trust-details-toggle" aria-expanded={open} aria-controls={id} onClick={() => setOpen(!open)}>
        <span className="context-row-glyph" aria-hidden="true">{open ? <ChevronDown size={ICON_SIZE.xs} /> : <ChevronRight size={ICON_SIZE.xs} />}</span>
        <span className="context-trust-details-label">Details</span>
        <span className="context-trust-details-summary">{detailsSummary(details)}</span>
      </button>
      <div id={id} hidden={!open}>{details.map((finding) => trustRow(finding))}</div>
    </div>
  );
}

type TrustReads = { key: string; trust: MessageTrust | null; familiarity: SenderFamiliarity | null };

/**
 * Reads still in flight, shared so the reader header and the Trust section
 * opening the same message ask once. Settled reads are not kept, so a later
 * look sees current history.
 */
const pendingTrustReads = new Map<string, Promise<TrustReads>>();

function readTrust(message: Message, key: string): Promise<TrustReads> {
  const pending = pendingTrustReads.get(key);
  if (pending) return pending;
  const from = parseAddress(message.sender);
  const failed = (what: string) => (reason: unknown) => {
    logBackgroundFailure(what)(reason);
    return null;
  };
  const read = Promise.all([
    mailClient.messageTrust(message.id).catch(failed("Message trust")),
    mailClient.senderFamiliarity(message.id, from.email, from.name).catch(failed("Sender familiarity")),
  ]).then(([trust, familiarity]) => ({ key, trust, familiarity }))
    .finally(() => pendingTrustReads.delete(key));
  pendingTrustReads.set(key, read);
  return read;
}

/**
 * The trust report for a message someone else sent, or null until both of
 * its reads finish (so nothing appears and then reorders) or while
 * `enabled` is false. The reader header and the Trust section both use it,
 * so they always agree.
 */
export function useTrustReport({ message, subject, bulk, prior, enabled = true }: {
  message: Message;
  subject: string;
  bulk: boolean;
  prior?: PriorThreadText;
  enabled?: boolean;
}): TrustReport | null {
  const [reads, setReads] = useState<TrustReads | null>(null);
  const key = `${message.id}\n${message.sender}`;
  useEffect(() => {
    if (!enabled) return;
    let active = true;
    readTrust(message, key).then((loaded) => { if (active) setReads(loaded); });
    return () => { active = false; };
    // `key` covers the message fields the reads use.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, key]);
  const links = useMemo(() => enabled ? analyzeLinks(message.bodyHtml, prior) : null, [enabled, message.bodyHtml, prior]);
  return useMemo(() => {
    if (!enabled || !links || reads?.key !== key) return null;
    return trustReport({ trust: reads.trust, links, familiarity: reads.familiarity, sender: message.sender, subject, attachments: message.attachments ?? [], bulk });
  }, [enabled, links, reads, key, message.sender, message.attachments, subject, bulk]);
}

/**
 * Whether the latest message from someone else can be trusted. A summary
 * heads the section: Be careful when anything warns, Known sender for
 * someone the user writes to or saved whose mail was verified. Rows give the
 * concrete tricks, such as a display name borrowed from a contact, a
 * look-alike domain or subject word, a link that hides where it goes, or a risky attachment,
 * and then the sender's history with the user; link rows can show their
 * link in the message. Sender checks, the sites links open, and tracking
 * images fold under Details. Bulk mail always shows the section; other
 * conversations only with a summary. Links are read only from the part shown
 * before folded quoted history; `prior` is the conversation text before the
 * message, as the reader folds it.
 */
export function TrustSection({ message, subject, bulk, prior, onShowLink }: {
  message: Message;
  subject: string;
  bulk: boolean;
  prior?: PriorThreadText;
  onShowLink?(link: TrustLink): void;
}) {
  const report = useTrustReport({ message, subject, bulk, prior });
  if (!report) return null;
  if (report.rows.length === 0 && report.details.length === 0) return null;
  if (!bulk && !report.summary) return null;
  const summary = report.summary === "careful"
    ? <span className="context-trust-summary context-trust-careful"><TriangleAlert size={ICON_SIZE.xs} aria-hidden="true" />Be careful</span>
    : report.summary === "known"
      ? <span className="context-trust-summary context-trust-known"><ShieldCheck size={ICON_SIZE.xs} aria-hidden="true" />Known sender</span>
      : null;
  // Familiar-sender reassurance stays compact; warnings retain their full section.
  if (report.summary === "known") return <section className="context-section context-trust context-trust-compact" aria-label="Trust">
    <details>
      <summary>{summary}<span className="context-trust-expand">Sender details</span></summary>
      {report.rows.map((finding) => trustRow(finding, onShowLink))}
      <TrustDetails key={message.id} details={report.details} />
    </details>
  </section>;
  return (
    <ContextSection
      id="trust"
      className="context-trust"
      title="Trust"
      label="Trust"
      actions={summary}
      limit={report.rows.length}
      rows={report.rows.map((finding) => trustRow(finding, onShowLink))}
      closing={<TrustDetails key={message.id} details={report.details} />}
    />
  );
}
