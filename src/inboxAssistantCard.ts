import type { InboxAgentProposal } from "./domain";
import { formatDisplayName, parseAddress } from "./emailAddress";

export const ASSISTANT_KIND_LABELS: Record<InboxAgentProposal["kind"], string> = {
  archive_with_summary: "Stale Read",
  archive_as_resolved: "Already Handled",
  archive_as_expired: "Expired",
  draft_reply: "Suggested Reply",
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
