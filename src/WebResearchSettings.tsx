import frozenPolicy from "../docs/agent-integrations/contract-v1.json";
import { CircleAlert, CircleCheck, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { SettingsToggle } from "./AiProviderSettings";
import { mailClient } from "./data/client";
import type { AgentResearchConfig, AgentResearchSettings } from "./domain";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";

type Provider = "model" | "search";

/**
 * Web research keeps its own Anthropic and Exa keys in the OS keychain,
 * separate from AI Provider. Every saved change here, including a key, clears
 * the connection check natively, so research cannot start on a configuration
 * that has not been checked.
 */
export function WebResearchSettings({ onChange }: { onChange?(): void }) {
  const [settings, setSettings] = useState<AgentResearchSettings | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [checking, setChecking] = useState(false);
  const [keys, setKeys] = useState<Record<Provider, string>>({ model: "", search: "" });
  const policy = settings?.policy ?? frozenPolicy;

  useEffect(() => {
    let active = true;
    void mailClient.agentResearchSettings()
      .then((next) => { if (active) setSettings(next); })
      .catch((reason: unknown) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, []);

  /** Resolves true once the change is saved and the page shows it. */
  const run = async (operation: () => Promise<void>): Promise<boolean> => {
    if (busy) return false;
    setBusy(true);
    setError("");
    try {
      await operation();
      setSettings(await mailClient.agentResearchSettings());
      onChange?.();
      return true;
    } catch (reason) {
      setError(errorMessage(reason));
      return false;
    } finally {
      setBusy(false);
    }
  };
  // Exa is required for web research, so every save keeps search on.
  const saveConfig = (patch: Partial<AgentResearchConfig>) => run(async () => {
    if (!settings) return;
    await mailClient.setAgentResearchSettings({ ...settings.config, searchEnabled: true, ...patch, verified: false });
  });

  if (!settings) {
    return (
      <section className="settings-section research-settings" aria-label="Web Research">
        <h3>Status</h3>
        {error ? <p key="error" className="form-error" role="alert">{error}</p> : <p key="loading" role="status">Loading web research settings…</p>}
      </section>
    );
  }

  const { config } = settings;
  const missing = [
    settings.modelKeyConfigured ? null : "an Anthropic API key",
    settings.searchKeyConfigured ? null : "an Exa API key",
  ].filter((item): item is string => item !== null);
  const canCheck = missing.length === 0 && !busy && !checking;
  const status = missing.length > 0
    ? { ready: false, text: `To finish setup, add ${listWords(missing)}.` }
    : !config.verified
      ? { ready: false, text: "Check connections to finish setup." }
      : !config.enabled
        ? { ready: false, text: "Connections checked. Turn on Web Research to use it." }
        : { ready: true, text: "Ready. Connections checked for this configuration." };

  const keyRow = (provider: Provider, label: string, configured: boolean) => (
    <>
      <div className="settings-field-row ai-key-row">
        <label className="settings-field settings-field-row">
          <span>{label}</span>
          <input
            type="password"
            autoComplete="new-password"
            value={keys[provider]}
            placeholder={configured ? "Saved to keychain" : "Paste API key"}
            onChange={(event) => setKeys((current) => ({ ...current, [provider]: event.target.value }))}
          />
        </label>
        <button
          className="btn"
          type="button"
          disabled={busy || !keys[provider].trim()}
          aria-label={`Save ${label}`}
          onClick={() => void run(() => mailClient.setAgentResearchKey(provider, keys[provider]))
            .then((saved) => { if (saved) setKeys((current) => ({ ...current, [provider]: "" })); })}
        >
          Save Key
        </button>
      </div>
      <div className="settings-field-detail ai-key-status">
        <span className={`settings-connection-status${configured ? " configured" : ""}`}>
          {configured ? <CircleCheck size={ICON_SIZE.xs} aria-hidden="true" /> : <CircleAlert size={ICON_SIZE.xs} aria-hidden="true" />}
          {configured ? "API key configured" : "API key required"}
        </span>
        {configured ? (
          <button
            type="button"
            className="btn-link"
            disabled={busy}
            aria-label={`Remove ${label}`}
            onClick={() => void run(() => mailClient.setAgentResearchKey(provider, ""))}
          >
            Remove Key
          </button>
        ) : null}
      </div>
    </>
  );

  return (
    <section className="settings-section research-settings" aria-label="Web Research">
      <h3>Status</h3>
      <p className="settings-hint">
        Delegated tasks in Agents can research the web with Claude Sonnet 4.6 and Exa search,
        using their own keys, separate from AI Provider. Jobs run only while this app is open and awake.
      </p>
      <p className={`settings-connection-status${status.ready ? " configured" : ""}`} role="status">
        {status.ready ? <CircleCheck size={ICON_SIZE.xs} aria-hidden="true" /> : <CircleAlert size={ICON_SIZE.xs} aria-hidden="true" />}
        {status.text}
      </p>
      {error ? <p className="form-error" role="alert">{error}</p> : null}

      <div className="settings-toggle-list">
        <SettingsToggle
          label="Web Research"
          description="Changing any setting on this page pauses running research and requires checking connections again."
          checked={config.enabled}
          disabled={busy}
          onChange={(value) => void saveConfig({ enabled: value })}
        />
      </div>

      <h3>Anthropic</h3>
      {keyRow("model", "Anthropic API Key", settings.modelKeyConfigured)}

      <h3>Exa Search</h3>
      {keyRow("search", "Exa API Key", settings.searchKeyConfigured)}

      <h3>Connections</h3>
      <div className="ai-connection-actions">
        <button
          className="btn"
          type="button"
          disabled={!canCheck}
          onClick={() => {
            setChecking(true);
            void run(() => mailClient.checkAgentResearchConnection()).finally(() => setChecking(false));
          }}
        >
          <RefreshCw size={ICON_SIZE.sm} aria-hidden="true" />
          {checking ? "Checking connections…" : "Check Connections"}
        </button>
      </div>
      <p className="settings-hint">
        Check connections sends one public test search to Exa, which may cost ${policy.search.usdPerRequest}, and
        checks model availability at Anthropic. It does not read your mail.
      </p>

      <h3>What Is Shared</h3>
      <ul className="research-disclosure-list">
        <li>Your objective, public subjects, follow-up messages, and retrieved evidence go to Anthropic.</li>
        <li>Exa receives only reviewed public subjects with fixed research terms.</li>
        <li>Reading a page contacts its host using your IP address.</li>
        <li>Raw evidence expires after seven days; reports and cited excerpts remain until you delete them.</li>
        <li>Providers retain data under your account agreement; this is not a zero-retention promise.</li>
      </ul>

      <details className="settings-disclosure">
        <summary>Limits and Estimated Cost</summary>
        <div className="settings-disclosure-body">
          <dl className="research-limits">
            <dt>Model calls</dt><dd>{policy.limits.modelCallsPerTask}</dd>
            <dt>Searches</dt><dd>{policy.limits.searchCallsPerTask}</dd>
            <dt>Pages read</dt><dd>{policy.limits.pageCallsPerTask}</dd>
            <dt>Input tokens</dt><dd>{policy.limits.inputTokensPerTask.toLocaleString()}</dd>
            <dt>Output tokens</dt><dd>{policy.limits.outputTokensPerTask.toLocaleString()}</dd>
            <dt>Active time</dt><dd>{policy.limits.activeSecondsPerTask / 60} minutes</dd>
          </dl>
          <p className="settings-hint">
            Limits apply per task. Retries and revisions share that allowance.
            Estimates ({policy.verifiedOn}): ${policy.model.inputUsdPerMillion}/${policy.model.outputUsdPerMillion} per
            million input/output model tokens and ${policy.search.usdPerRequest} per search; actual bills can differ.
            No exact dollar cap is promised.
          </p>
        </div>
      </details>
    </section>
  );
}

function listWords(items: string[]): string {
  if (items.length <= 1) return items.join("");
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}
