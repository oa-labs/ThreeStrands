import { useEffect, useMemo, useState } from "react";
import { Eye, Link2, Mail, Reply, Shield, ShieldCheck, TriangleAlert } from "lucide-react";
import type { Message, MessageTrust, SenderHistory, SplitInbox } from "./domain";
import { mailClient } from "./data/client";
import { ContextRow, ContextSection } from "./ContextSections";
import { formatHistoryDate } from "./contactContext";
import { decodeHtmlEntities } from "./SafeMessage";
import { ICON_SIZE } from "./iconSizes";
import { logBackgroundFailure } from "./errors";
import { analyzeLinks, describeSender, matchingSplit, SENDER_HISTORY_LIMIT, trustFindings, type TrustFinding } from "./senderSignals";

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
};

/**
 * What the user has received from a bulk sender in this account, and what to
 * do about it: unsubscribe, archive what is still in the Inbox, or give the
 * sender a split. Earlier conversations from the sender follow, since the
 * contact index (and so Recent emails) leaves automated mail out.
 */
export function SenderSection({ target, threadId, hideThreadIds, actions, onOpenThread }: {
  target: SenderTarget;
  /** The open conversation, which the rows leave out. */
  threadId: string;
  /** Conversations other sections already list. */
  hideThreadIds: string[];
  actions: SenderActions;
  onOpenThread(id: string): void;
}) {
  const { accountId, email } = target;
  const [history, setHistory] = useState<SenderHistory | null>(null);
  // Hides Archive once used, until the next load says what is left.
  const [archived, setArchived] = useState(false);
  useEffect(() => {
    let active = true;
    setHistory(null);
    setArchived(false);
    mailClient.senderHistory(accountId, email, SENDER_HISTORY_LIMIT)
      .then((loaded) => { if (active) setHistory(loaded); })
      .catch((reason: unknown) => {
        // The section is supplementary; a failed read leaves it out.
        if (active) setHistory(null);
        logBackgroundFailure("Sender history")(reason);
      });
    return () => { active = false; };
  }, [accountId, email, threadId]);
  if (!history || history.receivedCount === 0) return null;
  const facts = describeSender(history);
  const split = matchingSplit(actions.splits, accountId, email);
  if (split) facts.push(`Sorted into ${split.name}`);
  const inbox = archived ? [] : history.inboxThreadIds;
  const hidden = new Set([threadId, ...hideThreadIds]);
  const rows = history.threads.filter((item) => !hidden.has(item.threadId)).map((item) => {
    const snippet = decodeHtmlEntities(item.snippet).replace(/\s+/g, " ").trim();
    return (
      <ContextRow
        key={item.threadId}
        icon={<Mail size={ICON_SIZE.sm} />}
        title={item.subject || "(no subject)"}
        date={formatHistoryDate(item.sentAt)}
        detail={snippet || undefined}
        onActivate={() => onOpenThread(item.threadId)}
      />
    );
  });
  // Archiving one conversation is the reader's own Archive; this is for the rest.
  const canArchive = inbox.length > 1;
  const decisions = actions.onUnsubscribe || canArchive || !split ? (
    <div className="context-sender-actions">
      {actions.onUnsubscribe ? <button type="button" className="btn btn-sm" onClick={actions.onUnsubscribe}>Unsubscribe</button> : null}
      {canArchive ? (
        <button type="button" className="btn btn-sm" onClick={() => { setArchived(true); actions.onArchiveThreads(inbox); }}>
          Archive {inbox.length} in Inbox
        </button>
      ) : null}
      {split ? null : <button type="button" className="btn btn-sm" onClick={() => actions.onNewSplit(target)}>New Split…</button>}
    </div>
  ) : null;
  return (
    <ContextSection
      id="sender"
      className="context-sender"
      title={<>From <span className="context-domain">{email}</span></>}
      count={rows.length}
      note={facts.join(" · ")}
      lead={decisions}
      rows={rows}
    />
  );
}

const FINDING_ICONS: Record<TrustFinding["kind"], typeof Shield> = {
  authentication: Shield,
  replyTo: Reply,
  mismatch: Link2,
  sites: Link2,
  tracking: Eye,
};

function findingIcon(finding: TrustFinding) {
  if (finding.tone === "warning") return <TriangleAlert size={ICON_SIZE.sm} />;
  const Icon = finding.tone === "verified" ? ShieldCheck : FINDING_ICONS[finding.kind];
  return <Icon size={ICON_SIZE.sm} />;
}

/**
 * Whether the latest message from someone else can be trusted: the receiving
 * server's verdict on the sender, a Reply-To at another organization, links
 * whose text names another site, the sites links open, and tracking images.
 * Bulk mail always shows it; other conversations only when something needs a
 * look.
 */
export function TrustSection({ message, bulk }: { message: Message; bulk: boolean }) {
  const [trust, setTrust] = useState<{ id: string; value: MessageTrust | null } | null>(null);
  useEffect(() => {
    let active = true;
    mailClient.messageTrust(message.id)
      .then((value) => { if (active) setTrust({ id: message.id, value }); })
      .catch((reason: unknown) => {
        if (active) setTrust({ id: message.id, value: null });
        logBackgroundFailure("Message trust")(reason);
      });
    return () => { active = false; };
  }, [message.id]);
  const links = useMemo(() => analyzeLinks(message.bodyHtml), [message.bodyHtml]);
  // Wait for the headers so the section does not appear and then reorder.
  if (trust?.id !== message.id) return null;
  const findings = trustFindings(trust.value, links);
  if (findings.length === 0 || (!bulk && !findings.some((finding) => finding.tone === "warning"))) return null;
  return (
    <ContextSection
      id="trust"
      className="context-trust"
      title="Trust"
      count={findings.length}
      rows={findings.map((finding) => (
        <ContextRow
          key={finding.key}
          className={finding.tone === "warning" ? "context-row-warning" : finding.tone === "verified" ? "context-row-verified" : undefined}
          icon={findingIcon(finding)}
          title={finding.title}
          detail={finding.detail}
        />
      ))}
    />
  );
}
