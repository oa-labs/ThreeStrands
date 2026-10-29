import { EMAIL_TEXT_COLORS, EMAIL_ZOOM } from "./emailRenderingPolicy";
import { useCallback, useMemo, type Dispatch, type SetStateAction } from "react";
import type { useAccounts } from "./useAccounts";
import type { useAppPreferences } from "./useAppPreferences";
import type { useMailboxThreads } from "./useMailboxThreads";
import type { MailAccountSettings, SettingsSection } from "./settingsPanelTypes";
import type { SettingsImportResult } from "./userPreferences";
import { mailClient } from "./data/client";
import { DEFAULT_AVAILABILITY_PROFILES } from "./settings";

type Options = {
  mailAccounts: ReturnType<typeof useAccounts>;
  preferences: ReturnType<typeof useAppPreferences>;
  mailboxThreads: Pick<ReturnType<typeof useMailboxThreads>, "loadThreads" | "query">;
  refreshSplitInboxes: () => Promise<void>;
  refreshAiAvailability: () => void;
  setSettingsSection: Dispatch<SetStateAction<SettingsSection>>;
};

/** Adapts settings actions to account refresh and the live application preferences. */
export function useSettingsIntegration({
  mailAccounts, preferences, mailboxThreads, refreshSplitInboxes, refreshAiAvailability,
  setSettingsSection,
}: Options) {
  const {
    accounts, availableAccounts, activeAccountId, authStatus, addAccount, removeAccount, removeAccountEverywhere,
    reconnectAccount, setAccountDisplayName, setAccountNickname, setAccountColor, reorderAccounts, setActiveAccountId,
    refreshAccounts, setAuthStatus,
  } = mailAccounts;
  const {
    setTheme, setAccent, setFontScale, setFontFamily, setEmailMinimumFontSize, setEmailZoom, setEmailTextColors,
    setAutoReadDelaySeconds, setLoadRemoteImages, setAvailabilityProfiles,
  } = preferences;
  const { loadThreads, query } = mailboxThreads;
  // Removing or reconnecting an account changes which threads are visible,
  // so those two operations also reload the mailbox.
  const mailAccountSettings = useMemo((): MailAccountSettings => ({
    authStatus,
    accounts,
    availableAccounts,
    activeAccountId,
    add: addAccount,
    refresh: refreshAccounts,
    remove: async (email) => {
      const { wasActive } = await removeAccount(email);
      void loadThreads(query, wasActive ? null : undefined);
    },
    removeEverywhere: removeAccountEverywhere,
    reconnect: async (email) => {
      try {
        await reconnectAccount(email);
      } finally {
        await loadThreads(query);
      }
    },
    setDisplayName: setAccountDisplayName,
    setNickname: setAccountNickname,
    setColor: setAccountColor,
    reorder: reorderAccounts,
  }), [
    accounts, availableAccounts, activeAccountId, addAccount, authStatus, loadThreads, query, reconnectAccount,
    removeAccount, removeAccountEverywhere, reorderAccounts, setAccountColor,
    setAccountDisplayName, setAccountNickname, refreshAccounts,
  ]);

  const applyImportedSettings = useCallback(async ({ preferences: imported }: SettingsImportResult) => {
    setTheme(imported.theme);
    setAccent(imported.accent);
    setFontScale(imported.fontScale);
    setFontFamily(imported.fontFamily);
    setEmailMinimumFontSize(imported.emailMinimumFontSize ?? 0);
    setEmailZoom(imported.emailZoom ?? EMAIL_ZOOM.defaultPercent);
    setEmailTextColors(imported.emailTextColors ?? EMAIL_TEXT_COLORS.defaultMode);
    setAutoReadDelaySeconds(imported.autoReadDelaySeconds);
    setLoadRemoteImages(imported.loadRemoteImages);
    setAvailabilityProfiles(imported.availabilityProfiles ?? DEFAULT_AVAILABILITY_PROFILES, imported.availabilityPreferences);
    setActiveAccountId(imported.selectedAccountId);
    await Promise.all([
      refreshAccounts(),
      refreshSplitInboxes(),
      mailClient.googleAuthStatus().then(setAuthStatus),
    ]);
    refreshAiAvailability();
    setSettingsSection("accounts");
  }, [
    setTheme, setAccent, setFontScale, setFontFamily, setEmailMinimumFontSize, setEmailZoom, setEmailTextColors,
    setAutoReadDelaySeconds, setLoadRemoteImages, setAvailabilityProfiles, setActiveAccountId,
    refreshAccounts, refreshSplitInboxes, setAuthStatus, refreshAiAvailability, setSettingsSection,
  ]);

  return { mailAccountSettings, applyImportedSettings };
}
