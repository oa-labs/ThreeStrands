import { useEffect, useMemo, useState } from "react";
import { Eye, Link2, Reply, Shield, ShieldCheck, TriangleAlert } from "lucide-react";
import type { Message, MessageTrust, SenderHistory, SenderRule, SplitInbox } from "./domain";
import { mailClient } from "./data/client";
import { ContextRow, ContextSection } from "./ContextSections";
import { ICON_SIZE } from "./iconSizes";
import type { PriorThreadText } from "./quotedHistory";
import { logBackgroundFailure } from "./errors";
import { analyzeLinks, matchingRule, matchingSplit, SENDER_HISTORY_LIMIT, trustFindings, type TrustFinding } from "./senderSignals";

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
 * look. Links are read only from the part shown before folded quoted history;
 * `prior` is the conversation text before the message, as the reader folds it.
 */
export function TrustSection({ message, bulk, prior }: { message: Message; bulk: boolean; prior?: PriorThreadText }) {
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
  const links = useMemo(() => analyzeLinks(message.bodyHtml, prior), [message.bodyHtml, prior]);
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
