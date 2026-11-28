import { formatDateTime } from "./dateFormatting";
import { useEffect, useState } from "react";
import type { AiActivity } from "./domain";
import { mailClient } from "./data/client";
import { AI_PROVIDERS } from "./aiSettings";
import { formatTokens } from "./aiUsage";
import { errorMessage } from "./errors";
import { plural } from "./plural";

/** Entries fetched; the list shows the first few until asked for more. */
export const AI_ACTIVITY_LIMIT = 100;
const AI_ACTIVITY_SHOWN = 10;

const FEATURE_LABELS: Record<string, string> = {
  agent_stale_reads: "Stale Reads Summarizer",
  agent_resolved_sweeper: "Expired & Handled Sweeper",
  agent_aging_replies: "Aging Reply Drafter",
  agent_stale_reads_digest: "Stale Reads Digest",
  brief: "Brief",
  summary: "Brief",
  suggestions: "Suggestions",
  chat: "Ask About This Conversation",
  contact: "Enhance With AI",
  reply: "Draft Reply",
  draft_review: "Draft Review",
  find_times: "Find Times",
};

/** The name a feature id has in the app, such as "Brief" for `brief`. */
export function aiActivityFeatureLabel(feature: string): string {
  return FEATURE_LABELS[feature] ?? "Other";
}

function providerLabel(provider: string): string {
  return AI_PROVIDERS.find((candidate) => candidate.id === provider)?.label ?? provider;
}

/**
 * The most recent requests sent to an AI provider: what each was for, which
 * conversation, how many messages, and how large. Email text is never kept.
 */
export function AiActivityList() {
  const [entries, setEntries] = useState<AiActivity[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  useEffect(() => {
    let active = true;
    mailClient.aiActivity(AI_ACTIVITY_LIMIT)
      .then((result) => { if (active) setEntries(result); })
      .catch((reason: unknown) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, []);

  const shown = entries ? (showAll ? entries : entries.slice(0, AI_ACTIVITY_SHOWN)) : [];
  return <section className="ai-activity" aria-label="AI activity">
    <h3>Activity</h3>
    <p className="settings-hint">What was sent to your AI provider recently. Only these details are kept on this device, never the email text.</p>
    {error ? <p className="form-error" role="alert">{error}</p> : null}
    {!entries && !error ? <p className="settings-hint">Loading activity…</p> : null}
    {entries && entries.length === 0 ? <p className="settings-hint">Nothing has been sent yet.</p> : null}
    {shown.length > 0 ? <div className="ai-activity-scroll" role="region" aria-label="Activity table" tabIndex={0}>
      <table className="ai-activity-table" aria-label="Recent AI requests">
        <thead><tr>
          <th scope="col">Activity / Conversation</th>
          <th scope="col">When</th>
          <th scope="col">Provider / Model</th>
          <th scope="col" className="ai-activity-number">Messages</th>
          <th scope="col" className="ai-activity-number">Tokens</th>
        </tr></thead>
        <tbody>{shown.map((entry, index) => <tr key={`${entry.at}-${index}`}>
          <td>
            <strong>{aiActivityFeatureLabel(entry.feature)}</strong>
            {entry.subject ? <span className="ai-activity-subject">{entry.subject}</span> : null}
          </td>
          <td><time dateTime={entry.at}>{formatDateTime(new Date(entry.at))}</time></td>
          <td>
            <span className="ai-activity-provider">{providerLabel(entry.provider)}</span>
            <span className="ai-activity-model">{entry.model}</span>
          </td>
          <td className="ai-activity-number" aria-label={entry.messageCount !== null ? plural(entry.messageCount, "message") : "Not applicable"}>{entry.messageCount ?? "—"}</td>
          <td className="ai-activity-number" aria-label={`${formatTokens(entry.inputTokens + entry.outputTokens)} tokens`}>{formatTokens(entry.inputTokens + entry.outputTokens)}</td>
        </tr>)}</tbody>
      </table>
    </div> : null}
    {entries && entries.length > AI_ACTIVITY_SHOWN ? <button type="button" className="btn-link" onClick={() => setShowAll(!showAll)}>
      {showAll ? "Show Fewer" : `Show ${entries.length - AI_ACTIVITY_SHOWN} More`}
    </button> : null}
  </section>;
}
