import { describe, expect, it } from "vitest";
import type { Draft } from "./correspondence";
import {
  authoredText,
  composeChecks,
  draftRecipients,
  editDistance,
  knownAddressMap,
  likelyIntendedAddress,
  replaceAddress,
  type KnownCorrespondents,
} from "./composeChecks";
import type { ContactSuggestion } from "./domain";

const base: Draft = {
  id: "draft-1", revision: 0, account: "me@acme.com", mode: "new",
  sourceId: null, threadId: null, replyId: null, references: [],
  to: "", cc: "", bcc: "", subject: "Plans", body: "", attachments: [], updatedAt: 0,
};
const draft = (overrides: Partial<Draft>): Draft => ({ ...base, ...overrides });
const suggestion = (email: string, sentCount: number, receivedCount = 0, displayName: string | null = null, pinned = false): ContactSuggestion =>
  ({ email, displayName, sentCount, receivedCount, lastInteractedAt: "2026-09-01T00:00:00Z", pinned });
const known = (lists: Record<string, ContactSuggestion[]>): KnownCorrespondents =>
  Object.fromEntries(Object.entries(lists).map(([account, list]) => [account, knownAddressMap(list)]));
const own = ["me@acme.com", "me@gmail.com"];
const file = { id: "a1", name: "plan.pdf", size: 10, mime: "application/pdf", inline: false, ready: true } as Draft["attachments"][number];
const inlineImage = { ...file, id: "i1", inline: true, contentId: "img" } as Draft["attachments"][number];

describe("draftRecipients", () => {
  it("lists complete addresses once, To first, without the user's own or a half-typed one", () => {
    expect(draftRecipients(draft({
      to: "Ann Lee <Ann@Partner.com>, me@acme.com, bo",
      cc: "ann@partner.com, \"Doe, Jane\" <jane@other.org>",
      bcc: "x@y",
    }), own)).toEqual([
      { email: "ann@partner.com", name: "Ann Lee", field: "to" },
      { email: "jane@other.org", name: "Doe, Jane", field: "cc" },
    ]);
  });
});

describe("authoredText", () => {
  it("stops at a reply quote or a forwarded message", () => {
    expect(authoredText("Thanks!\n\nOn Mon, Ann <a@b.com> wrote:\n> see attached")).toBe("Thanks!");
    expect(authoredText("FYI\n\n---------- Forwarded message ----------\nFrom: x\n\nattached")).toBe("FYI");
    expect(authoredText("No quote here")).toBe("No quote here");
  });
});

describe("editDistance", () => {
  it("counts an adjacent swap as one edit and stops past the limit", () => {
    expect(editDistance("jonh", "john", 2)).toBe(1);
    expect(editDistance("jon", "john", 2)).toBe(1);
    expect(editDistance("gmial.com", "gmail.com", 2)).toBe(1);
    expect(editDistance("alice", "bob", 2)).toBe(3);
    expect(editDistance("a", "abcdef", 2)).toBe(3);
  });
});

describe("likelyIntendedAddress", () => {
  const entries = (list: ContactSuggestion[]) => knownAddressMap(list).entries();

  it("suggests a near address the user has written to, on the same domain or with the same name", () => {
    expect(likelyIntendedAddress("jonh@acme.com", entries([suggestion("john@acme.com", 3, 0, "John")]))).toEqual({ email: "john@acme.com", name: "John" });
    expect(likelyIntendedAddress("ann@partnr.com", entries([suggestion("ann@partner.com", 1)]))).toEqual({ email: "ann@partner.com", name: null });
  });

  it("never suggests a stranger, a different person on another domain, or a short name two edits away", () => {
    // Received from but never written to.
    expect(likelyIntendedAddress("jonh@acme.com", entries([suggestion("john@acme.com", 0, 5)]))).toBeNull();
    // Both parts differ.
    expect(likelyIntendedAddress("jon@acme.co", entries([suggestion("john@acme.com", 3)]))).toBeNull();
    // "ann" and "amy" are two edits apart; short names collide too easily.
    expect(likelyIntendedAddress("ann@acme.com", entries([suggestion("amy@acme.com", 3)]))).toBeNull();
  });

  it("allows two edits in a long name and counts a pinned address as known", () => {
    expect(likelyIntendedAddress("christophr@acme.com", entries([suggestion("christopher@acme.com", 0, 0, null, true)]))?.email).toBe("christopher@acme.com");
    expect(likelyIntendedAddress("chirstophr@acme.com", entries([suggestion("christopher@acme.com", 2)]))?.email).toBe("christopher@acme.com");
  });

  it("prefers the closest match, then the one written to most", () => {
    expect(likelyIntendedAddress("jonh@acme.com", entries([
      suggestion("joan@acme.com", 10),
      suggestion("john@acme.com", 1),
    ]))?.email).toBe("john@acme.com");
    expect(likelyIntendedAddress("sam@acme.com", entries([
      suggestion("pam@acme.com", 1),
      suggestion("sal@acme.com", 9),
    ]))?.email).toBe("sal@acme.com");
  });
});

describe("composeChecks", () => {
  it("flags a promised attachment only in the user's own words and only without a real attachment", () => {
    expect(composeChecks({ draft: draft({ body: "I've attached the plan." }), ownEmails: own, known: null }))
      .toContainEqual({ kind: "attachment", word: "attached" });
    expect(composeChecks({ draft: draft({ body: "Plan enclosed.", attachments: [inlineImage] }), ownEmails: own, known: null }))
      .toContainEqual({ kind: "attachment", word: "enclosed" });
    expect(composeChecks({ draft: draft({ body: "I've attached the plan.", attachments: [file] }), ownEmails: own, known: null })
      .some((check) => check.kind === "attachment")).toBe(false);
    expect(composeChecks({ draft: draft({ mode: "reply", body: "Thanks\n\nOn Mon, Ann <a@b.com> wrote:\n> See attached" }), ownEmails: own, known: null })
      .some((check) => check.kind === "attachment")).toBe(false);
    // Discussing the other person's attachment is not a promise.
    expect(composeChecks({ draft: draft({ body: "Your attachment didn't come through." }), ownEmails: own, known: null })
      .some((check) => check.kind === "attachment")).toBe(false);
  });

  it("flags a missing subject", () => {
    expect(composeChecks({ draft: draft({ subject: "  " }), ownEmails: own, known: null })).toEqual([{ kind: "subject" }]);
  });

  it("leaves out history checks until correspondents are loaded", () => {
    expect(composeChecks({ draft: draft({ to: "stranger@else.com" }), ownEmails: own, known: null })).toEqual([]);
  });

  it("offers the likely intended address for a typo and marks a true first email", () => {
    const history = known({ "me@acme.com": [suggestion("john@partner.com", 4, 2, "John Roe")] });
    expect(composeChecks({ draft: draft({ to: "jonh@partner.com, new@else.com" }), ownEmails: own, known: history })).toEqual([
      { kind: "typo", email: "jonh@partner.com", suggestion: "john@partner.com", suggestionName: "John Roe" },
      { kind: "firstContact", email: "new@else.com" },
    ]);
  });

  it("treats anyone with history in any account as known, and keeps first-email notes out of replies", () => {
    const history = known({ "me@acme.com": [], "me@gmail.com": [suggestion("ann@partner.com", 0, 3)] });
    expect(composeChecks({ draft: draft({ to: "ann@partner.com" }), ownEmails: own, known: history })).toEqual([]);
    expect(composeChecks({ draft: draft({ mode: "replyAll", to: "ann@partner.com", cc: "bystander@partner.com" }), ownEmails: own, known: history })).toEqual([]);
  });

  it("suggests the other account when every known recipient was written to only from it", () => {
    const history = known({
      "me@acme.com": [suggestion("pal@friends.org", 0, 2)],
      "me@gmail.com": [suggestion("pal@friends.org", 6), suggestion("bud@friends.org", 2)],
    });
    expect(composeChecks({ draft: draft({ to: "pal@friends.org, bud@friends.org" }), ownEmails: own, known: history }))
      .toEqual([{ kind: "account", account: "me@gmail.com", emails: ["pal@friends.org", "bud@friends.org"] }]);
    // Written to from this account too: no suggestion.
    const both = known({ "me@acme.com": [suggestion("pal@friends.org", 1)], "me@gmail.com": [suggestion("pal@friends.org", 6)] });
    expect(composeChecks({ draft: draft({ to: "pal@friends.org" }), ownEmails: own, known: both })).toEqual([]);
    // Only a new message can change accounts.
    expect(composeChecks({ draft: draft({ mode: "reply", to: "pal@friends.org" }), ownEmails: own, known: history })).toEqual([]);
  });

  it("flags outsiders on a mostly internal message, but not introductions or personal accounts", () => {
    const internal = draft({ to: "a@acme.com, b@acme.com", cc: "client@partner.com" });
    expect(composeChecks({ draft: internal, ownEmails: own, known: null }))
      .toEqual([{ kind: "outside", domain: "acme.com", emails: ["client@partner.com"] }]);
    // One colleague and one outsider is an ordinary introduction.
    expect(composeChecks({ draft: draft({ to: "a@acme.com, client@partner.com" }), ownEmails: own, known: null })).toEqual([]);
    // A personal mailbox has no organization to be outside of.
    expect(composeChecks({ draft: { ...internal, account: "me@gmail.com", to: "a@gmail.com, b@gmail.com" }, ownEmails: own, known: null })).toEqual([]);
  });
});

describe("replaceAddress", () => {
  it("replaces only the matching entry and keeps the rest as written", () => {
    expect(replaceAddress("Ann <ann@x.com>, JONH@acme.com, ", "jonh@acme.com", "John <john@acme.com>"))
      .toBe("Ann <ann@x.com>, John <john@acme.com>");
    expect(replaceAddress("ann@x.com", "bob@x.com", "rob@x.com")).toBe("ann@x.com");
  });
});
