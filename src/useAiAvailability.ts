import { useCallback, useEffect, useState } from "react";
import { AI_PROVIDERS, isAiApiKeyConfigured, readAiFeatures, readAiProvider, type AiProvider } from "./aiSettings";

/** How the Brief names who a conversation was shared with. */
export function aiProviderName(provider: AiProvider): string | null {
  if (provider === "none") return null;
  if (provider === "custom") return "your custom endpoint";
  return AI_PROVIDERS.find((candidate) => candidate.id === provider)?.label ?? null;
}

/**
 * Which AI features to show. A feature is *enabled* when an AI provider is
 * chosen and its switch is on, and hidden otherwise. It is *available* when
 * an API key is also configured; enabled but unavailable features show how
 * to finish setting up instead of their actions.
 */
export function useAiAvailability(settingsOpen: boolean) {
  const [aiProviderChosen, setAiProviderChosen] = useState(false);
  const [aiProviderLabel, setAiProviderLabel] = useState<string | null>(null);
  const [aiSummaryAvailable, setAiSummaryAvailable] = useState(false);
  const [aiSummaryFeatureEnabled, setAiSummaryFeatureEnabled] = useState(false);
  const [aiProactive, setAiProactive] = useState<{ enabled: boolean; knownSendersOnly: boolean }>({ enabled: false, knownSendersOnly: false });
  const [aiActionFeatureEnabled, setAiActionFeatureEnabled] = useState(false);
  const [aiActionAvailable, setAiActionAvailable] = useState(false);
  const [aiChatFeatureEnabled, setAiChatFeatureEnabled] = useState(false);
  const [aiChatAvailable, setAiChatAvailable] = useState(false);
  const [aiDraftFeatureEnabled, setAiDraftFeatureEnabled] = useState(false);
  const [aiDraftAvailable, setAiDraftAvailable] = useState(false);
  const refreshAiAvailability = useCallback(() => {
    const provider = readAiProvider();
    const features = readAiFeatures();
    const summaryEnabled = provider !== "none" && features.summarize;
    const actionEnabled = provider !== "none" && features.actionExtraction;
    const chatEnabled = provider !== "none" && features.threadChat;
    const draftEnabled = provider !== "none" && features.draftAssist;
    setAiProviderChosen(provider !== "none");
    setAiProviderLabel(aiProviderName(provider));
    setAiChatFeatureEnabled(chatEnabled);
    setAiSummaryFeatureEnabled(summaryEnabled);
    setAiProactive({ enabled: features.proactiveBriefs, knownSendersOnly: features.proactiveKnownSendersOnly });
    setAiActionFeatureEnabled(actionEnabled);
    setAiDraftFeatureEnabled(draftEnabled);
    if (!summaryEnabled && !actionEnabled && !chatEnabled && !draftEnabled) {
      setAiSummaryAvailable(false);
      setAiActionAvailable(false);
      setAiChatAvailable(false);
      setAiDraftAvailable(false);
      return;
    }
    void isAiApiKeyConfigured()
      .then((configured) => {
        setAiSummaryAvailable(configured && summaryEnabled);
        setAiActionAvailable(configured && actionEnabled);
        setAiChatAvailable(configured && chatEnabled);
        setAiDraftAvailable(configured && draftEnabled);
      })
      .catch(() => {
        setAiSummaryAvailable(false);
        setAiActionAvailable(false);
        setAiChatAvailable(false);
        setAiDraftAvailable(false);
      });
  }, []);
  useEffect(() => {
    // Also covers the initial mount, since `settingsOpen` starts `false`.
    if (!settingsOpen) refreshAiAvailability();
  }, [settingsOpen, refreshAiAvailability]);
  return {
    aiProviderChosen, aiProviderLabel, aiSummaryAvailable, aiSummaryFeatureEnabled, aiProactive, aiActionFeatureEnabled,
    aiActionAvailable, aiChatFeatureEnabled, aiChatAvailable, aiDraftFeatureEnabled, aiDraftAvailable, refreshAiAvailability,
  };
}
