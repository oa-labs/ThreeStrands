import { useCallback, useEffect, useState } from "react";
import { isAiApiKeyConfigured, readAiFeatures, readAiProvider } from "./aiSettings";

export function useAiAvailability(settingsOpen: boolean) {
  const [aiSummaryAvailable, setAiSummaryAvailable] = useState(false);
  const [aiSummaryFeatureEnabled, setAiSummaryFeatureEnabled] = useState(false);
  const [aiProactive, setAiProactive] = useState<{ enabled: boolean; knownSendersOnly: boolean }>({ enabled: false, knownSendersOnly: false });
  const [aiActionFeatureEnabled, setAiActionFeatureEnabled] = useState(false);
  const [aiActionAvailable, setAiActionAvailable] = useState(false);
  const [aiChatFeatureEnabled, setAiChatFeatureEnabled] = useState(false);
  const [aiChatAvailable, setAiChatAvailable] = useState(false);
  const refreshAiAvailability = useCallback(() => {
    const provider = readAiProvider();
    const features = readAiFeatures();
    const summaryEnabled = provider !== "none" && features.summarize;
    const actionEnabled = provider !== "none" && features.actionExtraction;
    const chatEnabled = provider !== "none" && features.threadChat;
    setAiChatFeatureEnabled(features.threadChat);
    setAiSummaryFeatureEnabled(features.summarize);
    setAiProactive({ enabled: features.proactiveBriefs, knownSendersOnly: features.proactiveKnownSendersOnly });
    setAiActionFeatureEnabled(features.actionExtraction);
    if (!summaryEnabled && !actionEnabled && !chatEnabled) {
      setAiSummaryAvailable(false);
      setAiActionAvailable(false);
      setAiChatAvailable(false);
      return;
    }
    void isAiApiKeyConfigured()
      .then((configured) => {
        setAiSummaryAvailable(configured && summaryEnabled);
        setAiActionAvailable(configured && actionEnabled);
        setAiChatAvailable(configured && chatEnabled);
      })
      .catch(() => {
        setAiSummaryAvailable(false);
        setAiActionAvailable(false);
        setAiChatAvailable(false);
      });
  }, []);
  useEffect(() => {
    // Also covers the initial mount, since `settingsOpen` starts `false`.
    if (!settingsOpen) refreshAiAvailability();
  }, [settingsOpen, refreshAiAvailability]);
  return {
    aiSummaryAvailable, aiSummaryFeatureEnabled, aiProactive, aiActionFeatureEnabled,
    aiActionAvailable, aiChatFeatureEnabled, aiChatAvailable, refreshAiAvailability,
  };
}
