import { formatFullDate } from "./dateFormatting";
import type { InboxAgentProposal } from "./domain";
import { formatDisplayName, parseAddress } from "./emailAddress";

export const ASSISTANT_KIND_LABELS: Record<InboxAgentProposal["kind"], string> = {
  archive_with_summary: "Stale Read",
  archive_as_resolved: "Already Handled",
  archive_as_expired: "Expired",
  draft_reply: "Suggested Reply",
  sender_hygiene: "Ignored Sender",
  digest_item: "Digest",
};

/** Extracts a concise sender display label and exposes the raw email in tooltip only when it adds info. */
export function formatProposalSender(raw: string): { label: string; title?: string } {
  const trimmed = raw.trim();
  if (!trimmed) return { label: "Unknown sender" };
  const parsed = parseAddress(trimmed);
  const display = formatDisplayName(parsed.name);
  const label = display || parsed.email || trimmed;
  const email = parsed.email.trim();
  const title = email && label.toLowerCase() !== email.toLowerCase() ? email : undefined;
  return { label, ...(title ? { title } : {}) };
}

/** Returns clean plain-text takeaway bullets, falling back to legacy prose summary lines when needed. */
export function proposalTakeaways(proposal: InboxAgentProposal): string[] {
  const structured = proposal.payload.takeaways
    ?.map((item) => item.replace(/^[•\-*]\s*/, "").trim())
    .filter((item) => item.length > 0);
  if (structured && structured.length > 0) return structured;

  const summary = proposal.payload.summary?.trim();
  if (!summary) return [];
  return summary
    .split(/\r?\n+/)
    .map((line) => line.replace(/^[•\-*]\s*/, "").trim())
    .filter((line) => line.length > 0);
}

/** Suppresses redundant subject sublines when the AI headline already matches or wraps the subject. */
export function shouldShowSubjectSubline(headline: string, subject: string): boolean {
  const normalizedSubject = subject.trim().toLowerCase().replace(/\s+/g, " ");
  if (!normalizedSubject) return false;
  const normalizedHeadline = headline.trim().toLowerCase().replace(/\s+/g, " ");
  return Boolean(normalizedHeadline) && normalizedHeadline !== normalizedSubject;
}

/** Builds clean Markdown body text when saving a proposal to Notes or prefilling a new Task. */
export function formatProposalNoteBody(proposal: InboxAgentProposal): string {
  const sections: string[] = [];
  const deadline = proposal.payload.deadline?.trim();
  if (deadline) sections.push(`Deadline: ${deadline}`);
  const takeaways = proposalTakeaways(proposal);
  if (takeaways.length > 0) {
    sections.push(takeaways.map((item) => `- ${item}`).join("\n"));
  } else {
    const fallback = proposal.payload.summary?.trim() || proposal.reason.trim();
    if (fallback) sections.push(fallback);
  }
  return sections.join("\n\n");
}

const TIMESTAMP_EVIDENCE = /^(Completed|Marked done)\s+(\d{4}-\d{2}-\d{2}T\S+)$/i;

/** Formats deterministic task completion ISO timestamps cleanly for user display. */
export function formatEvidenceExcerpt(excerpt: string): string {
  const match = TIMESTAMP_EVIDENCE.exec(excerpt.trim());
  if (!match) return excerpt;
  const parsed = new Date(match[2]);
  if (!Number.isFinite(parsed.getTime())) return excerpt;
  return `Marked done ${formatFullDate(parsed)}`;
}

/** Limits one-click bulk archive to expired items or tasks explicitly marked done by the user. */
export function isSafeForBatchArchive(proposal: InboxAgentProposal): boolean {
  if (proposal.kind === "archive_as_expired") return true;
  return (
    proposal.kind === "archive_as_resolved" &&
    proposal.payload.resolvedByKind === "task" &&
    proposal.payload.taskCompletionSource === "user"
  );
}

/** "Today", "1 day old", or "N days old" for a received-at timestamp; empty when unparseable. */
export function formatAgeDays(iso: string): string {
  const ts = Date.parse(iso);
  if (!Number.isFinite(ts)) return "";
  const days = Math.max(0, Math.floor((Date.now() - ts) / 86_400_000));
  if (days === 0) return "Today";
  if (days === 1) return "1 day old";
  return `${days} days old`;
}

/** The digest's gist for one item: its summary, else first takeaway, else the reason. */
export function digestGist(proposal: InboxAgentProposal): string {
  return proposal.payload.summary?.trim() || proposalTakeaways(proposal)[0] || proposal.reason;
}
