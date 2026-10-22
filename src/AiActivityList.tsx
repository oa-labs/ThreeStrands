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
    {shown.length > 0 ? <ul className="ai-activity-list">
      {shown.map((entry, index) => {
        const when = new Date(entry.at).toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
        const details = [
          when,
          `${providerLabel(entry.provider)} ${entry.model}`,
          entry.messageCount !== null ? plural(entry.messageCount, "message") : null,
          `${formatTokens(entry.inputTokens + entry.outputTokens)} tokens`,
        ].filter(Boolean).join(" · ");
        return <li key={`${entry.at}-${index}`}>
          <span className="ai-activity-title">
            <strong>{aiActivityFeatureLabel(entry.feature)}</strong>
            {entry.subject ? <span className="ai-activity-subject">{entry.subject}</span> : null}
          </span>
          <small>{details}</small>
        </li>;
      })}
    </ul> : null}
    {entries && entries.length > AI_ACTIVITY_SHOWN ? <button type="button" className="btn-link" onClick={() => setShowAll(!showAll)}>
      {showAll ? "Show Fewer" : `Show ${entries.length - AI_ACTIVITY_SHOWN} More`}
    </button> : null}
  </section>;
}
