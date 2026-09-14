import { describe, expect, it } from "vitest";
import { formatLabelName, labelIdsForConversationDisplay, sortLabelIdsForDisplay } from "./labels";

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
