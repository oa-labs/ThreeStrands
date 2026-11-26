import { useEffect, useState } from "react";
import { PauseCircle, Play, Settings2, Sparkles } from "lucide-react";
import { AccountScopeChip } from "./AccountScope";
import { isEditableTarget } from "./commands";
import { formatDateTime } from "./dateFormatting";
import type { Account, InboxAgentConfig, InboxAgentId, InboxAgentProposal } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import {
  ASSISTANT_KIND_LABELS,
  formatProposalSender,
  proposalTakeaways,
  shouldShowSubjectSubline,
} from "./inboxAssistantCard";
import { InboxAgentConfigModal } from "./InboxAgentConfigModal";
import { readCurrentAiRunConfig, type InboxAgentsController } from "./useInboxAgents";
import { useWorkspaceCommands } from "./workspaceCommands";

function formatAgeDays(iso: string): string {
  const ts = Date.parse(iso);
  if (!Number.isFinite(ts)) return "";
  const days = Math.max(0, Math.floor((Date.now() - ts) / 86_400_000));
  if (days === 0) return "Today";
  if (days === 1) return "1 day old";
  return `${days} days old`;
}

export function AgentsWorkspace({
  controller,
  accounts,
  accountId,
  onOpenThread,
  onOpenTask,
  onArchiveProposal,
  onArchiveBatch,
  onSaveToNoteAndArchive,
  onCreateTask,
  onReviewDraft,
  onOpenAiSettings,
}: {
  controller: InboxAgentsController;
  accounts: readonly Account[];
  accountId: string | null;
  onOpenThread(threadId: string): void;
  onOpenTask(taskId: string): void;
  onArchiveProposal(proposal: InboxAgentProposal): void;
  onArchiveBatch(proposals: InboxAgentProposal[]): void;
  onSaveToNoteAndArchive(proposal: InboxAgentProposal): void;
  onCreateTask(proposal: InboxAgentProposal): void;
  onReviewDraft(proposal: InboxAgentProposal, draftText: string): void;
  onOpenAiSettings(): void;
}) {
  const { snapshot, loaded, busy, error } = controller;
  const [filterAgent, setFilterAgent] = useState<InboxAgentId | "all">("all");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editingConfig, setEditingConfig] = useState<InboxAgentConfig | null>(null);

  const visibleProposals = snapshot.proposals.filter(
    (p) => filterAgent === "all" || p.agentId === filterAgent,
  );
  const safeBatch = visibleProposals.filter(
    (p) => p.kind === "archive_as_expired" || p.kind === "archive_as_resolved",
  );
  const activeProposal =
    visibleProposals.find((p) => p.id === selectedId) ?? visibleProposals[0] ?? null;

  useEffect(() => {
    if (visibleProposals.length === 0) {
      setSelectedId(null);
    } else if (!selectedId || !visibleProposals.some((p) => p.id === selectedId)) {
      setSelectedId(visibleProposals[0].id);
    }
  }, [selectedId, visibleProposals]);

  useWorkspaceCommands({
    "agents.runAll": () => { void controller.runEnabled(); },
  });

  const aiConfigured = Boolean(readCurrentAiRunConfig());
  const anyEnabled = snapshot.agents.some((a) => a.enabled);
  const anyRunning = snapshot.activeRuns.length > 0;
  const showAccount = accountId === null && accounts.filter((a) => a.onThisDevice !== false).length > 1;

  return (
    <section
      className="agents-workspace"
      aria-label="Inbox Assistants"
      onKeyDown={(e) => {
        if (isEditableTarget(e.target) || editingConfig || visibleProposals.length === 0) return;
        if (e.key === "j" || e.key === "ArrowDown") {
          e.preventDefault();
          const idx = visibleProposals.findIndex((p) => p.id === activeProposal?.id);
          const next = visibleProposals[Math.min(visibleProposals.length - 1, Math.max(0, idx) + 1)];
          if (next) setSelectedId(next.id);
          return;
        }
        if (e.key === "k" || e.key === "ArrowUp") {
          e.preventDefault();
          const idx = visibleProposals.findIndex((p) => p.id === activeProposal?.id);
          const prev = visibleProposals[Math.max(0, Math.max(0, idx) - 1)];
          if (prev) setSelectedId(prev.id);
          return;
        }
        if (!activeProposal) return;
        if (e.key === "e") {
          e.preventDefault();
          onArchiveProposal(activeProposal);
        } else if (e.key === "n" && activeProposal.kind === "archive_with_summary") {
          e.preventDefault();
          onSaveToNoteAndArchive(activeProposal);
        } else if (e.key === "r" && activeProposal.kind === "draft_reply" && activeProposal.payload.replyDraft) {
          e.preventDefault();
          onReviewDraft(activeProposal, activeProposal.payload.replyDraft);
        } else if (e.key === "x") {
          e.preventDefault();
          void controller.resolveProposal(activeProposal.id, "dismissed");
        } else if (e.key === "o" || e.key === "Enter") {
          if (e.target instanceof HTMLButtonElement) return;
          e.preventDefault();
          onOpenThread(activeProposal.threadId);
        }
      }}
    >
      <header className="agents-toolbar">
        <h1>Inbox Assistants</h1>
        <AccountScopeChip accountId={accountId} />
        <button
          type="button"
          className="btn btn-primary"
          disabled={busy || !snapshot.runnerEnabled || !anyEnabled}
          onClick={() => void controller.runEnabled()}
        >
          <Play size={ICON_SIZE.sm} aria-hidden="true" />
          {anyRunning ? "Running…" : "Run Enabled"}
        </button>
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => void controller.toggleRunner(!snapshot.runnerEnabled)}
        >
          {snapshot.runnerEnabled ? "Pause All" : "Resume All"}
        </button>
        <button type="button" className="btn" disabled={busy} onClick={() => void controller.refresh()}>
          Refresh
        </button>
      </header>

      {!snapshot.runnerEnabled ? (
        <div className="agents-paused" role="status">
          <PauseCircle size={ICON_SIZE.sm} aria-hidden="true" />
          <span><strong>Inbox assistants are paused.</strong> Scheduled and manual runs will not start until resumed.</span>
          <button type="button" className="btn" disabled={busy} onClick={() => void controller.toggleRunner(true)}>
            Resume All
          </button>
        </div>
      ) : null}

      {!aiConfigured ? (
        <div className="agents-paused" role="status">
          <Sparkles size={ICON_SIZE.sm} aria-hidden="true" />
          <span>
            <strong>AI provider not configured.</strong> Completed-task checks run locally, while summaries, cross-thread checks, and reply drafts require an AI provider.
          </span>
          <button type="button" className="btn" onClick={onOpenAiSettings}>AI Provider Settings</button>
        </div>
      ) : null}

      {error ? (
        <p className="form-error assistant-error-banner" role="alert">
          {error}{" "}
          <button type="button" className="btn-link" onClick={() => void controller.refresh()}>
            Try Again
          </button>
        </p>
      ) : null}

      <div className="agents-layout">
        <aside className="agent-list" aria-label="Assistants and filters">
          <div className="assistant-filter-group" role="group" aria-label="Filter suggestions">
            <button
              type="button"
              className={`assistant-filter-btn ${filterAgent === "all" ? "selected" : ""}`}
              aria-pressed={filterAgent === "all"}
              onClick={() => setFilterAgent("all")}
            >
              <span>All Suggestions</span>
              <span className="assistant-count-badge">{snapshot.proposals.length}</span>
            </button>
            {snapshot.agents.map((agent) => {
              const count = snapshot.proposals.filter((p) => p.agentId === agent.agentId).length;
              return (
                <button
                  key={agent.agentId}
                  type="button"
                  className={`assistant-filter-btn ${filterAgent === agent.agentId ? "selected" : ""}`}
                  aria-pressed={filterAgent === agent.agentId}
                  onClick={() => setFilterAgent(agent.agentId)}
                >
                  <span>{agent.name}</span>
                  <span className="assistant-count-badge">{count}</span>
                </button>
              );
            })}
          </div>

          <h2 className="eyebrow assistant-section-heading">Assistants</h2>
          <div className="assistant-config-list">
            {snapshot.agents.map((agent) => {
              const running = snapshot.activeRuns.some((r) => r.agentId === agent.agentId);
              return (
                <article className="assistant-config-card" key={agent.agentId} aria-label={agent.name}>
                  <div className="assistant-config-head">
                    <strong>{agent.name}</strong>
                    <span className={`assistant-status-pill ${agent.enabled ? "enabled" : ""}`}>
                      {running ? "Running…" : agent.enabled ? "Enabled" : "Off"}
                    </span>
                  </div>
                  <p className="agent-hint">{agent.description}</p>
                  <p className="assistant-config-meta">
                    Older than {agent.ageDays}d · Up to {agent.maxThreadsPerRun} emails
                    {agent.scheduleHours === 24 ? " · Daily" : agent.scheduleHours === 168 ? " · Weekly" : " · Manual"}
                  </p>
                  {agent.lastRunAt ? (
                    <p className="assistant-config-meta">
                      Last run {formatDateTime(new Date(agent.lastRunAt * 1000))}:{" "}
                      {agent.lastRunStatus === "failed"
                        ? agent.lastRunError || "Failed"
                        : `${agent.lastRunScanned} scanned, ${agent.lastRunProposed} suggested`}
                    </p>
                  ) : null}
                  <div className="assistant-config-actions">
                    <button
                      type="button"
                      className="btn btn-sm"
                      disabled={busy}
                      onClick={() =>
                        void controller.updateConfig({
                          agentId: agent.agentId,
                          enabled: !agent.enabled,
                          ageDays: agent.ageDays,
                          maxThreadsPerRun: agent.maxThreadsPerRun,
                          includeStarred: agent.includeStarred,
                          accountId: agent.accountId,
                          scheduleHours: agent.scheduleHours,
                        })
                      }
                    >
                      {agent.enabled ? "Disable" : "Enable"}
                    </button>
                    <button
                      type="button"
                      className="btn btn-sm"
                      disabled={busy || !snapshot.runnerEnabled || running}
                      onClick={() => void controller.runAgent(agent.agentId)}
                    >
                      {running ? "Running…" : "Run Now"}
                    </button>
                    <button
                      type="button"
                      className="btn btn-sm"
                      disabled={busy}
                      aria-label={`Configure ${agent.name}`}
                      onClick={() => setEditingConfig(agent)}
                    >
                      <Settings2 size={ICON_SIZE.xs} aria-hidden="true" />
                      Configure…
                    </button>
                  </div>
                </article>
              );
            })}
          </div>
        </aside>

        <div className="assistant-queue" role="region" aria-label="Inbox suggestions">
          {safeBatch.length >= 2 ? (
            <div className="assistant-batch-bar">
              <span>{safeBatch.length} expired or already-handled emails ready to clear</span>
              <button
                type="button"
                className="btn btn-sm btn-primary"
                disabled={busy}
                onClick={() => onArchiveBatch(safeBatch)}
              >
                Archive All {safeBatch.length} Safe
              </button>
            </div>
          ) : null}

          {!loaded ? (
            <p className="assistant-empty" role="status">Loading inbox assistants…</p>
          ) : visibleProposals.length === 0 ? (
            <div className="agent-empty">
              <h2>No pending suggestions</h2>
              <p>
                Enable one or more assistants on the left and choose <strong>Run Now</strong> (or set a schedule) to review aging reads, handled threads, and unanswered emails. Nothing is archived or sent without your review.
              </p>
            </div>
          ) : (
            <div className="assistant-proposal-list">
              {visibleProposals.map((proposal) => {
                const isSelected = activeProposal?.id === proposal.id;
                const linkedThread = proposal.evidence.find((e) => e.kind === "thread");
                const linkedTask = proposal.evidence.find((e) => e.kind === "task");
                const evidenceExcerpt = proposal.evidence.find((e) => e.excerpt)?.excerpt;
                const isSummary = proposal.kind === "archive_with_summary";
                const sender = formatProposalSender(proposal.threadSender);
                const ageLabel = formatAgeDays(proposal.threadLastReceivedAt);
                const deadline = proposal.payload.deadline?.trim();
                const takeaways = isSummary ? proposalTakeaways(proposal) : [];
                const primaryHeading = proposal.headline || proposal.threadSubject || "(No subject)";
                return (
                  <article
                    key={proposal.id}
                    className={`assistant-proposal-card ${isSelected ? "selected" : ""}`}
                    aria-label={proposal.threadSubject || primaryHeading}
                    onClick={() => setSelectedId(proposal.id)}
                  >
                    <header className="assistant-proposal-header">
                      <span className="assistant-kind-badge">{ASSISTANT_KIND_LABELS[proposal.kind]}</span>
                      <span className="assistant-meta-sep" aria-hidden="true">·</span>
                      <span className="assistant-sender-label" title={sender.title}>{sender.label}</span>
                      {ageLabel ? (
                        <>
                          <span className="assistant-meta-sep" aria-hidden="true">·</span>
                          <span className="assistant-age-label">{ageLabel}</span>
                        </>
                      ) : null}
                      {showAccount ? (
                        <>
                          <span className="assistant-meta-sep" aria-hidden="true">·</span>
                          <span className="assistant-account-label">{proposal.accountId}</span>
                        </>
                      ) : null}
                      {deadline ? <span className="assistant-deadline-badge">Deadline: {deadline}</span> : null}
                    </header>
                    <h3 className="assistant-proposal-subject">{primaryHeading}</h3>
                    {shouldShowSubjectSubline(proposal.headline, proposal.threadSubject) ? (
                      <p className="assistant-proposal-subline">{proposal.threadSubject}</p>
                    ) : null}
                    {isSummary ? (
                      takeaways.length > 0 ? (
                        <ul className="assistant-takeaways">
                          {takeaways.map((item, idx) => (
                            <li key={idx}>{item}</li>
                          ))}
                        </ul>
                      ) : null
                    ) : (
                      <>
                        <p className="assistant-card-reason">{proposal.reason}</p>
                        {evidenceExcerpt ? (
                          <blockquote className="assistant-evidence-quote">{evidenceExcerpt}</blockquote>
                        ) : null}
                      </>
                    )}
                    {proposal.payload.replyDraft ? (
                      <div className="assistant-card-preview">
                        <span className="assistant-preview-label">Suggested reply:</span>
                        {proposal.payload.replyDraft}
                      </div>
                    ) : null}
                    <div className="assistant-card-actions">
                      {proposal.kind === "archive_with_summary" ? (
                        <>
                          <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => onArchiveProposal(proposal)}>
                            Archive
                          </button>
                          {proposal.payload.suggestSaveToNote === true ? (
                            <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onSaveToNoteAndArchive(proposal)}>
                              Save to Notes &amp; Archive
                            </button>
                          ) : null}
                          <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onCreateTask(proposal)}>
                            Create Task
                          </button>
                        </>
                      ) : null}
                      {(proposal.kind === "archive_as_resolved" || proposal.kind === "archive_as_expired") ? (
                        <>
                          <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => onArchiveProposal(proposal)}>
                            Archive
                          </button>
                          {linkedThread ? (
                            <button type="button" className="btn btn-sm" onClick={() => onOpenThread(linkedThread.id)}>
                              View Resolving Thread
                            </button>
                          ) : null}
                          {linkedTask ? (
                            <button type="button" className="btn btn-sm" onClick={() => onOpenTask(linkedTask.id)}>
                              View Completed Task
                            </button>
                          ) : null}
                        </>
                      ) : null}
                      {proposal.kind === "draft_reply" && proposal.payload.replyDraft ? (
                        <>
                          <button
                            type="button"
                            className="btn btn-sm btn-primary"
                            disabled={busy}
                            onClick={() => onReviewDraft(proposal, proposal.payload.replyDraft!)}
                          >
                            Review Draft
                          </button>
                          {proposal.payload.gentleOutDraft ? (
                            <button
                              type="button"
                              className="btn btn-sm"
                              disabled={busy}
                              onClick={() => onReviewDraft(proposal, proposal.payload.gentleOutDraft!)}
                            >
                              Quick Check-In
                            </button>
                          ) : null}
                          <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onArchiveProposal(proposal)}>
                            Archive
                          </button>
                        </>
                      ) : null}
                      <button type="button" className="btn btn-sm" onClick={() => onOpenThread(proposal.threadId)}>
                        Open Conversation
                      </button>
                      <button
                        type="button"
                        className="btn-link assistant-dismiss-btn"
                        disabled={busy}
                        onClick={() => void controller.resolveProposal(proposal.id, "dismissed")}
                      >
                        Dismiss
                      </button>
                    </div>
                  </article>
                );
              })}
            </div>
          )}
        </div>
      </div>

      {editingConfig ? (
        <InboxAgentConfigModal
          config={editingConfig}
          accounts={accounts}
          busy={busy}
          onSave={controller.updateConfig}
          onClose={() => setEditingConfig(null)}
        />
      ) : null}
    </section>
  );
}
