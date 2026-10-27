import { listen } from "@tauri-apps/api/event";
import { useEffect, useId, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Eye, Globe, Link2, Mail, Paperclip, Reply, Shield, ShieldCheck, TriangleAlert, UserCheck, UserRound } from "lucide-react";
import type { Message, MessageTrust, SenderFamiliarity, SenderHistory, SenderRule, SplitInbox } from "./domain";
import { parseAddress } from "./emailAddress";
import { mailClient } from "./data/client";
import { ContextRow, ContextSection } from "./ContextSections";
import { ICON_SIZE } from "./iconSizes";
import type { PriorThreadText } from "./quotedHistory";
import { errorMessage, logBackgroundFailure } from "./errors";
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
  delivery: UserCheck,
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
const SENDER_FEEDBACK_CHANGED = "threestrands:sender-feedback-changed";

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
  const [revision, setRevision] = useState(0);
  const key = `${message.id}\n${message.sender}`;
  useEffect(() => {
    const refresh = () => { pendingTrustReads.clear(); setRevision((value) => value + 1); };
    let active = true;
    let stop: (() => void) | undefined;
    window.addEventListener(SENDER_FEEDBACK_CHANGED, refresh);
    if ("__TAURI_INTERNALS__" in window) {
      void listen("replicated-sync-status", refresh).then((unlisten) => {
        if (active) stop = unlisten;
        else unlisten();
      }).catch(logBackgroundFailure("Sender confirmation sync listener"));
    }
    return () => { active = false; stop?.(); window.removeEventListener(SENDER_FEEDBACK_CHANGED, refresh); };
  }, []);
  useEffect(() => {
    if (!enabled) return;
    let active = true;
    readTrust(message, key).then((loaded) => { if (active) setReads(loaded); });
    return () => { active = false; };
    // `key` covers the message fields the reads use.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, key, revision]);
  const links = useMemo(() => enabled ? analyzeLinks(message.bodyHtml, prior) : null, [enabled, message.bodyHtml, prior]);
  return useMemo(() => {
    if (!enabled || !links || reads?.key !== key) return null;
    return trustReport({ messageId: message.id, trust: reads.trust, links, familiarity: reads.familiarity, sender: message.sender, subject, attachments: message.attachments ?? [], bulk });
  }, [enabled, links, reads, key, message.id, message.sender, message.attachments, subject, bulk]);
}

/**
 * A user assertion about this request, with an explicit scope and reversible
 * storage. `compact` leaves out the list of remembered senders, for the
 * warning above the message body.
 */
export function SenderFeedback({ messageId, feedback, compact = false }: { messageId: string; feedback: NonNullable<TrustReport["feedback"]>; compact?: boolean }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async (forgetId?: string) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      if (forgetId) await mailClient.forgetSenderDelivery(forgetId);
      else await mailClient.confirmSenderDelivery(messageId, feedback.linkOrigins);
      window.dispatchEvent(new Event(SENDER_FEEDBACK_CHANGED));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };
  const matchingId = feedback.matching?.id;
  const scopeId = useId();
  return (
    <div className="context-sender-feedback">
      <p id={scopeId} className="context-sender-feedback-scope">
        Trust {matchingId ? "applies" : "will apply"} to this sending address and its current link destinations for this mail account.
        {feedback.replyTo ? <> Includes replies to {feedback.replyTo}.</> : null} Authentication failures and suspicious links still warn.
      </p>
      <div className="context-sender-actions">
        {matchingId
          ? <button type="button" className="btn-link" disabled={busy} onClick={() => { void save(matchingId); }}>Forget confirmation</button>
          : <button type="button" className="btn btn-sm btn-wrap" aria-describedby={scopeId} disabled={busy} onClick={() => { void save(); }}>{busy ? "Saving…" : "Trust this address for this account"}</button>}
      </div>
      {!compact && !matchingId && feedback.patterns.length > 0 ? (
        <details>
          <summary>Remembered senders</summary>
          {feedback.patterns.map((pattern) => <div className="context-sender-actions" key={pattern.id}><span>{pattern.senderEmail}</span><button type="button" className="btn-link" disabled={busy} aria-label={`Forget confirmation for ${pattern.senderEmail}`} onClick={() => { void save(pattern.id); }}>Forget</button></div>)}
        </details>
      ) : null}
      {error ? <p role="alert">Couldn’t save sender feedback: {error}</p> : null}
    </div>
  );
}

/**
 * Sender checks for the latest message from someone else when nothing warns:
 * a remembered confirmation to forget, a request whose Reply-To may need a
 * confirmation, or bulk mail from anyone but a known sender. Warnings live
 * in the reader, where the badge beside the sender and the banner above the
 * body name them and the badge's details offer the trust action; a known
 * sender's shield offers its details the same way. Rows give the sender's
 * history with the user; sender checks, the sites links open, and tracking
 * images fold under Details. Links are read only from the part shown before
 * folded quoted history; `prior` is the conversation text before the
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
  if (!report || report.summary === "careful") return null;
  if (report.rows.length === 0 && report.details.length === 0) return null;
  if (!report.feedback?.relevant && (report.summary === "known" || (!bulk && !report.summary))) return null;
  return (
    // The id keeps the collapse state the section had under its earlier Trust title.
    <ContextSection
      id="trust"
      className="context-trust"
      title="Sender checks"
      label="Sender checks"
      limit={report.rows.length}
      rows={report.rows.map((finding) => trustRow(finding, onShowLink))}
      closing={<><TrustDetails key={message.id} details={report.details} />{report.feedback ? <SenderFeedback key={message.id} messageId={message.id} feedback={report.feedback} /> : null}</>}
    />
  );
}
