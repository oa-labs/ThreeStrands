import { describe, expect, it } from "vitest";
import {
  conversationLabelGroups,
  formatLabelName,
  isManageableLabel,
  labelIdsForConversationDisplay,
  sortLabelIdsForDisplay,
} from "./labels";

describe("formatLabelName", () => {
  it("strips the CATEGORY_ prefix from Gmail's system category labels", () => {
    expect(formatLabelName({ name: "CATEGORY_UPDATES" })).toBe("UPDATES");
    expect(formatLabelName({ name: "CATEGORY_SOCIAL" })).toBe("SOCIAL");
    expect(formatLabelName({ name: "CATEGORY_PROMOTIONS" })).toBe("PROMOTIONS");
    expect(formatLabelName({ name: "CATEGORY_FORUMS" })).toBe("FORUMS");
    expect(formatLabelName({ name: "CATEGORY_PERSONAL" })).toBe("PERSONAL");
  });

  it("leaves other label names untouched", () => {
    expect(formatLabelName({ name: "INBOX" })).toBe("INBOX");
    expect(formatLabelName({ name: "Work" })).toBe("Work");
  });
});

describe("sortLabelIdsForDisplay", () => {
  it("puts INBOX first, category labels next, then other labels in their original order", () => {
    const ids = ["important-clients", "CATEGORY_PERSONAL", "INBOX", "STARRED", "CATEGORY_UPDATES"];
    expect(sortLabelIdsForDisplay(ids)).toEqual([
      "INBOX",
      "CATEGORY_PERSONAL",
      "CATEGORY_UPDATES",
      "important-clients",
      "STARRED",
    ]);
  });

  it("is a no-op when there's no INBOX or category label", () => {
    const ids = ["work", "family"];
    expect(sortLabelIdsForDisplay(ids)).toEqual(["work", "family"]);
  });

  it("handles an empty list", () => {
    expect(sortLabelIdsForDisplay([])).toEqual([]);
  });
});

describe("labelIdsForConversationDisplay", () => {
  it("hides message-level state while retaining conversation labels", () => {
    expect(labelIdsForConversationDisplay([
      "SENT",
      "client",
      "STARRED",
      "INBOX",
      "UNREAD",
      "CATEGORY_PERSONAL",
      "DRAFT",
      "IMPORTANT",
    ])).toEqual(["INBOX", "CATEGORY_PERSONAL", "client", "IMPORTANT"]);
  });
});

describe("isManageableLabel", () => {
  it("includes every user label", () => {
    expect(isManageableLabel({ id: "Label_1", kind: "user" })).toBe(true);
  });

  it("includes system labels without a dedicated control, like categories and IMPORTANT", () => {
    expect(isManageableLabel({ id: "CATEGORY_UPDATES", kind: "system" })).toBe(true);
    expect(isManageableLabel({ id: "CATEGORY_PERSONAL", kind: "system" })).toBe(true);
    expect(isManageableLabel({ id: "IMPORTANT", kind: "system" })).toBe(true);
  });

  it("excludes system labels that already have a dedicated control elsewhere", () => {
    for (const id of ["INBOX", "SENT", "DRAFT", "TRASH", "SPAM", "UNREAD", "STARRED", "CHAT"]) {
      expect(isManageableLabel({ id, kind: "system" })).toBe(false);
    }
  });
});

describe("conversationLabelGroups", () => {
  const catalog = [
    { id: "INBOX", name: "Inbox", kind: "system" as const },
    { id: "CATEGORY_PERSONAL", name: "CATEGORY_PERSONAL", kind: "system" as const },
    { id: "Label_1", name: "Todo", kind: "user" as const },
    { id: "Label_2", name: "Personal", kind: "user" as const },
  ];

  it("separates system labels from user labels, formatting both by name", () => {
    const result = conversationLabelGroups(["INBOX", "CATEGORY_PERSONAL", "Label_1", "Label_2"], catalog);
    expect(result.systemLabelNames).toEqual(["Inbox", "PERSONAL"]);
    expect(result.userLabels).toEqual([
      { id: "Label_1", name: "Todo", kind: "user" },
      { id: "Label_2", name: "Personal", kind: "user" },
    ]);
  });

  it("drops an unresolved opaque user-label id instead of flashing it, matching a thread with no user labels", () => {
    for (const labelIds of [["INBOX", "Label_99"], ["INBOX"]]) {
      const result = conversationLabelGroups(labelIds, catalog);
      expect(result.systemLabelNames).toEqual(["Inbox"]);
      expect(result.userLabels).toEqual([]);
    }
  });

  it("falls back to the raw id for an unresolved non-opaque id", () => {
    const result = conversationLabelGroups(["some-custom-id"], catalog);
    expect(result.systemLabelNames).toEqual(["some-custom-id"]);
    expect(result.userLabels).toEqual([]);
  });

  it("handles a missing catalog by falling back to raw system ids", () => {
    const result = conversationLabelGroups(["INBOX", "Label_1"], undefined);
    expect(result.systemLabelNames).toEqual(["INBOX"]);
    expect(result.userLabels).toEqual([]);
  });
});
