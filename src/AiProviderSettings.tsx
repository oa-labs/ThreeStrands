import { CircleAlert, CircleCheck, RefreshCw } from "lucide-react";
import { useEffect, useRef, useId, useState } from "react";
import {
  AI_MODEL_PLACEHOLDERS,
  AI_MODEL_SUGGESTIONS,
  AI_PROVIDER_OPTIONS,
  clearAiApiKey,
  isAiApiKeyConfigured,
  readAiEndpoint,
  readAiFastModel,
  readAiFeatures,
  readAiModel,
  readAiProvider,
  resolveAiModel,
  saveAiEndpoint,
  saveAiFastModel,
  saveAiFeatures,
  saveAiModel,
  saveAiProvider,
  setAiApiKey,
  testAiConnection,
  type AiFeatureFlags,
  type AiProvider,
} from "./aiSettings";
import { errorMessage } from "./errors";
import { AiUsageSummary } from "./AiUsageSummary";
import { MIN_PROACTIVE_DWELL_SECONDS } from "./proactiveBrief";
import { ICON_SIZE } from "./iconSizes";

/**
 * `onChange` must run after every saved field, not just the switches: it
 * queues the portable preference record, and the next replicated-sync pull
 * applies that record over local storage, so a field saved without it is
 * silently reverted to its last queued value.
 */
export function AiProviderSettings({ onChange }: { onChange?: () => void }) {
  const [provider, setProvider] = useState(readAiProvider);
  const [model, setModel] = useState(readAiModel);
  const [fastModel, setFastModel] = useState(readAiFastModel);
  const [endpoint, setEndpoint] = useState(readAiEndpoint);
  const [features, setFeatures] = useState<AiFeatureFlags>(readAiFeatures);
  const featuresRef = useRef(features);
  const [keyConfigured, setKeyConfigured] = useState(false);
  const [keyInput, setKeyInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [configurationError, setConfigurationError] = useState<string | null>(null);
  const [testingConnection, setTestingConnection] = useState(false);
  const [connectionTested, setConnectionTested] = useState(false);

  useEffect(() => {
    void isAiApiKeyConfigured().then(setKeyConfigured);
  }, []);

  const updateFeature = (flag: keyof AiFeatureFlags, value: boolean) => {
    const next = { ...featuresRef.current, [flag]: value };
    featuresRef.current = next;
    saveAiFeatures(next);
    setFeatures(next);
    onChange?.();
  };

  const brief = features.summarize || features.actionExtraction;
  const testDisabled = busy || testingConnection || !keyConfigured || !resolveAiModel(provider, model)
    || (provider === "custom" && !endpoint.trim());

  return (
    <section className="settings-section ai-settings" aria-label="AI provider">
      <p className="settings-hint">
        Disabled by default. ThreeStrands only sends thread content to your chosen
        provider for the features you turn on below, using your own API key.
      </p>

      <h3>Connection</h3>
      <label className="settings-field settings-field-row">
        <span>Provider</span>
        <select
          value={provider}
          onChange={(event) => {
            const next = event.target.value as AiProvider;
            setProvider(next);
            setConnectionTested(false);
            saveAiProvider(next);
            onChange?.();
          }}
        >
          {AI_PROVIDER_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>{option.label}</option>
          ))}
        </select>
      </label>

      {provider !== "none" ? (
        <>
          {provider === "custom" ? (
            <label className="settings-field settings-field-row">
              <span>Endpoint URL</span>
              <input
                value={endpoint}
                placeholder="https://api.example.com/v1"
                onChange={(event) => {
                  setEndpoint(event.target.value);
                  setConnectionTested(false);
                  saveAiEndpoint(event.target.value);
                  onChange?.();
                }}
              />
            </label>
          ) : null}

          <div className="settings-field-row ai-key-row">
            <label className="settings-field settings-field-row">
              <span>API Key</span>
              <input
                type="password"
                value={keyInput}
                placeholder={keyConfigured ? "Saved to keychain" : "Paste API key"}
                onChange={(event) => setKeyInput(event.target.value)}
              />
            </label>
            <button className="btn"
              type="button"
              disabled={busy || !keyInput.trim()}
              onClick={() => {
                setBusy(true);
                setConfigurationError(null);
                void setAiApiKey(keyInput)
                  .then(() => {
                    setKeyInput("");
                    return isAiApiKeyConfigured();
                  })
                  .then(setKeyConfigured)
                  .then(() => onChange?.())
                  .catch((reason: unknown) => setConfigurationError(errorMessage(reason)))
                  .finally(() => setBusy(false));
              }}
            >
              Save Key
            </button>
          </div>
          <div className="settings-field-detail ai-key-status">
            <span className={`settings-connection-status${keyConfigured ? " configured" : ""}`} role="status">
              {keyConfigured ? <CircleCheck size={ICON_SIZE.xs} aria-hidden="true" /> : <CircleAlert size={ICON_SIZE.xs} aria-hidden="true" />}
              {keyConfigured ? "API key configured" : "API key required"}
            </span>
            <span className="settings-hint">Stored in your OS keychain, never in the mail database.</span>
            {keyConfigured ? (
              <button
                type="button"
                className="btn-link"
                disabled={busy}
                onClick={() => {
                  setBusy(true);
                  setConfigurationError(null);
                  void clearAiApiKey()
                    .then(() => isAiApiKeyConfigured())
                    .then(setKeyConfigured)
                    .then(() => onChange?.())
                    .catch((reason: unknown) => setConfigurationError(errorMessage(reason)))
                    .finally(() => setBusy(false));
                }}
              >
                Remove Key
              </button>
            ) : null}
          </div>
          <div className="settings-field-detail ai-connection-actions">
            <button
              className="btn"
              type="button"
              disabled={testDisabled}
              onClick={() => {
                setTestingConnection(true);
                setConfigurationError(null);
                setConnectionTested(false);
                void testAiConnection(provider, resolveAiModel(provider, model), endpoint)
                  .then(() => setConnectionTested(true))
                  .catch((reason: unknown) => setConfigurationError(errorMessage(reason)))
                  .finally(() => setTestingConnection(false));
              }}
            >
              <RefreshCw size={ICON_SIZE.sm} aria-hidden="true" />
              {testingConnection ? "Testing connection…" : "Test Connection"}
            </button>
            {connectionTested ? <span className="settings-connection-status configured" role="status"><CircleCheck size={ICON_SIZE.xs} aria-hidden="true" /> Connection successful</span> : null}
          </div>
          {configurationError ? <p className="form-error settings-field-detail" role="alert">{configurationError}</p> : null}

          <h3>Models</h3>
          <label className="settings-field settings-field-row">
            <span>Reasoning model</span>
            <input
              value={model}
              placeholder={AI_MODEL_PLACEHOLDERS[provider]}
              onChange={(event) => {
                setModel(event.target.value);
                setConnectionTested(false);
                saveAiModel(event.target.value);
                onChange?.();
              }}
            />
          </label>
          <div className="settings-field-detail">
            {AI_MODEL_SUGGESTIONS[provider].length > 0 ? (
              <div className="model-suggestions" aria-label="Suggested models">
                {AI_MODEL_SUGGESTIONS[provider].map((suggestion) => (
                  <button
                    key={suggestion}
                    type="button"
                    className={`model-suggestion${model.trim() === suggestion ? " selected" : ""}`}
                    onClick={() => {
                      setModel(suggestion);
                      setConnectionTested(false);
                      saveAiModel(suggestion);
                      onChange?.();
                    }}
                  >
                    {suggestion}
                  </button>
                ))}
              </div>
            ) : null}
            <p className="settings-hint">
              Used for suggestions, briefs, and conversation chat, where working through
              dates and multi-step questions pays off.
            </p>
          </div>

          <label className="settings-field settings-field-row">
            <span>Fast model</span>
            <input
              value={fastModel}
              placeholder="Same as reasoning model"
              onChange={(event) => {
                setFastModel(event.target.value);
                saveAiFastModel(event.target.value);
                onChange?.();
              }}
            />
          </label>
          <p className="settings-hint settings-field-detail">
            Optional. Used for summaries, reply drafts, and contact enrichment, which mostly
            read and copy text. Leave blank to use the reasoning model for everything.
          </p>

          <h3>Features</h3>
          <div className="settings-toggle-list">
            <SettingsToggle
              label="Draft Assist"
              description="Reviews your drafts and drafts replies from a short instruction."
              checked={features.draftAssist}
              onChange={(value) => updateFeature("draftAssist", value)}
            />
            <SettingsToggle
              label="Thread Summaries"
              description="Summarizes the open conversation."
              checked={features.summarize}
              onChange={(value) => updateFeature("summarize", value)}
            />
            <SettingsToggle
              label="Suggestions"
              description="Suggests next steps, such as tasks and meetings, for the open conversation."
              checked={features.actionExtraction}
              onChange={(value) => updateFeature("actionExtraction", value)}
            />
            <SettingsToggle
              label="Proactive Suggestions"
              description={`Prepares the brief once you stay on a conversation for the mark-read delay, at least ${MIN_PROACTIVE_DWELL_SECONDS} seconds. They skip mailing lists and conversations with no one else in them, and read each conversation again only when a new message arrives.${brief ? "" : " Turn on Thread Summaries or Suggestions first."}`}
              checked={features.proactiveBriefs}
              disabled={!brief}
              onChange={(value) => updateFeature("proactiveBriefs", value)}
            />
            <SettingsToggle
              label="Only for People I’ve Emailed"
              description="Limits proactive suggestions to senders you have written to."
              checked={features.proactiveKnownSendersOnly}
              disabled={!features.proactiveBriefs || !brief}
              nested
              onChange={(value) => updateFeature("proactiveKnownSendersOnly", value)}
            />
            <SettingsToggle
              label="Thread Chat"
              description="Answers questions about the open conversation. Press q or ⌘J to ask; Escape returns to shortcuts. It shares other emails only for a question where you choose Search all mail."
              checked={features.threadChat}
              onChange={(value) => updateFeature("threadChat", value)}
            />
            <SettingsToggle
              label="Contact Enrichment"
              description="Starts with three local emails. If they yield no supported suggestions, it checks up to nine more. You can choose to search more emails when the first three yield suggestions."
              checked={features.contactEnrichment}
              onChange={(value) => updateFeature("contactEnrichment", value)}
            />
          </div>

          <AiUsageSummary provider={provider} model={resolveAiModel(provider, model)} fastModel={fastModel} />
        </>
      ) : null}
    </section>
  );
}

/** A switch whose accessible name stays the short label while its longer
 * explanation is attached as a description, so the row reads as one unit
 * without the explanation floating loose below unrelated switches. */
function SettingsToggle({
  label,
  description,
  checked,
  disabled = false,
  nested = false,
  onChange,
}: {
  label: string;
  description?: string;
  checked: boolean;
  disabled?: boolean;
  nested?: boolean;
  onChange(value: boolean): void;
}) {
  const descriptionId = useId();
  return (
    <label className={`settings-switch settings-toggle${nested ? " nested" : ""}${disabled ? " disabled" : ""}`}>
      <span className="settings-toggle-text">
        <span className="settings-toggle-label">{label}</span>
        {description ? <span className="settings-toggle-description" id={descriptionId}>{description}</span> : null}
      </span>
      <input
        type="checkbox"
        aria-label={label}
        aria-describedby={description ? descriptionId : undefined}
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
    </label>
  );
}
