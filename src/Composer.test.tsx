import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Composer } from "./Composer";
import type { Draft } from "./correspondence";
import { mailClient } from "./data/client";
import type { Account } from "./domain";
import {
  clearAiApiKey,
  DEFAULT_AI_FEATURES,
  saveAiFeatures,
  saveAiProvider,
  setAiApiKey,
} from "./aiSettings";

const draft: Draft = {
  id: "draft-1",
  revision: 0,
  account: "first@example.com",
  mode: "new",
  sourceId: null,
  threadId: null,
  replyId: null,
  references: [],
  to: "",
  cc: "",
  bcc: "",
  subject: "",
  body: "",
  attachments: [],
  updatedAt: 0,
};

const accounts: Account[] = ["first@example.com", "second@example.com"].map((email, sortOrder) => ({
  email,
  displayName: null,
  color: "#4285F4",
  status: "connected",
  sortOrder,
  connectedAt: "2026-01-01T00:00:00Z",
  lastSyncedAt: null,
}));

describe("Composer From selector", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("keeps native pointer interaction and changes the sending account inline", async () => {
    vi.spyOn(mailClient, "setDraftAccount").mockResolvedValue({
      ...draft,
      revision: 1,
      account: "second@example.com",
    });

    render(<Composer draft={draft} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);
    const selector = screen.getByRole("combobox", { name: "Send from" });

    expect(fireEvent.pointerDown(selector, { button: 0, pointerId: 1 })).toBe(true);
    fireEvent.change(selector, { target: { value: "second@example.com" } });

    await waitFor(() => expect(selector).toHaveValue("second@example.com"));
    expect(mailClient.setDraftAccount).toHaveBeenCalledWith("draft-1", "second@example.com");
  });
});

describe("Composer pasted images", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("inserts, resizes, and removes an image pasted into the message body", async () => {
    const inlineAttachment = { id: "inline-1", name: "screenshot.png", mime: "image/png", size: 4, ready: true, messageId: null, providerId: null, inline: true, contentId: "inline-1@dispatch.local" };
    const attachInline = vi.spyOn(mailClient, "attachInlineImage").mockResolvedValue({ ...draft, revision: 1, attachments: [inlineAttachment] });
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const removeAttachment = vi.spyOn(mailClient, "removeAttachment").mockResolvedValue({ ...draft, revision: 3 });
    render(<Composer draft={draft} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message body" });
    Object.defineProperty(editor, "clientWidth", { configurable: true, value: 800 });
    const imageFile = new File([new Uint8Array([137, 80, 78, 71])], "screenshot.png", { type: "image/png" });

    fireEvent.paste(editor, {
      clipboardData: { files: [imageFile], getData: () => "" },
    });

    const image = await waitFor(() => {
      const pasted = editor.querySelector<HTMLImageElement>('img[alt="screenshot.png"]');
      expect(pasted).toBeInTheDocument();
      return pasted!;
    });
    expect(attachInline).toHaveBeenCalledWith("draft-1", "screenshot.png", "image/png", expect.any(String));
    const handle = screen.getByRole("slider", { name: "Resize pasted image" });
    fireEvent.pointerDown(handle, { clientX: 100 });
    fireEvent.pointerMove(window, { clientX: 160 });
    fireEvent.pointerUp(window);
    expect(image).toHaveAttribute("width", "380");
    await waitFor(() => expect(saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      bodyHtml: expect.stringContaining('src="cid:inline-1@dispatch.local"'),
    })));

    fireEvent.click(screen.getByRole("button", { name: "Remove pasted image" }));
    expect(editor.querySelector("img")).not.toBeInTheDocument();
    await waitFor(() => expect(removeAttachment).toHaveBeenCalledWith("draft-1", "inline-1"));
  });

  it("restores an inline image preview when a saved draft is reopened", async () => {
    const savedDraft = {
      ...draft,
      bodyHtml: '<p>See below</p><img src="cid:inline-1@dispatch.local" alt="Screenshot" width="320">',
      attachments: [{ id: "inline-1", name: "screenshot.png", mime: "image/png", size: 4, ready: true, messageId: null, providerId: null, inline: true, contentId: "inline-1@dispatch.local" }],
    };
    const readInline = vi.spyOn(mailClient, "readInlineImage").mockResolvedValue("data:image/png;base64,iVBORw==");

    render(<Composer draft={savedDraft} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);

    const image = screen.getByRole("img", { name: "Screenshot" });
    await waitFor(() => expect(image).toHaveAttribute("src", "data:image/png;base64,iVBORw=="));
    expect(readInline).toHaveBeenCalledWith("draft-1", "inline-1");
    expect(screen.getByRole("button", { name: "Remove pasted image" })).toBeInTheDocument();
  });
});

describe("Composer forwarded attachments", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("downloads unresolved forwarded images automatically before sending", async () => {
    const image = {
      id: "forwarded-image",
      name: "image003.jpg",
      mime: "image/jpeg",
      size: 4096,
      ready: false,
      messageId: "source-message",
      providerId: "provider-image",
      inline: false,
      contentId: null,
    };
    const forwarded = {
      ...draft,
      mode: "forward" as const,
      to: "friend@example.com",
      attachments: [image],
    };
    const ready = {
      ...forwarded,
      revision: 1,
      attachments: [{ ...image, ready: true }],
    };
    const fetchAttachment = vi.spyOn(mailClient, "fetchAttachment").mockResolvedValue(ready);
    const queued = {
      id: "queued-forward",
      draft: ready,
      state: "undo_pending" as const,
      deadline: Date.now() + 10_000,
      error: null,
    };
    const queueDraft = vi.spyOn(mailClient, "queueDraft").mockResolvedValue(queued);
    const onQueued = vi.fn();

    render(<Composer draft={forwarded} accounts={accounts} onClose={() => {}} onQueued={onQueued} />);

    await waitFor(() => expect(fetchAttachment).toHaveBeenCalledWith("draft-1", "forwarded-image"));
    await waitFor(() => expect(screen.getByText(/Ready/)).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: /Send/ }));

    await waitFor(() => expect(queueDraft).toHaveBeenCalledWith("draft-1", 1));
    expect(onQueued).toHaveBeenCalledWith(queued);
  });
});

describe("Composer Reply Assist", () => {
  const replyDraft: Draft = {
    ...draft,
    mode: "reply",
    sourceId: "source-message",
    threadId: "provider-thread",
    to: "sender@example.com",
    subject: "Project timing",
    body: "\n\nOn Sep 16, Sender wrote:\n> Can we meet Friday?",
  };
  const context = {
    subject: "Project timing",
    messages: [{
      sender: "Sender <sender@example.com>",
      sentAt: "2026-09-16T12:00:00Z",
      bodyText: "Can we meet Friday?",
    }],
  };

  beforeEach(async () => {
    saveAiProvider("openai");
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, draftAssist: true });
    await setAiApiKey("test-key");
  });

  afterEach(async () => {
    cleanup();
    vi.restoreAllMocks();
    await clearAiApiKey();
    saveAiProvider("none");
    saveAiFeatures(DEFAULT_AI_FEATURES);
  });

  it("shows the exact reviewed context and inserts provider output only as plain text", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const generate = vi.spyOn(mailClient, "generateReply").mockResolvedValue({
      body: '<img src=x onerror="alert(1)">Friday works for me.',
    });
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));

    render(<Composer draft={replyDraft} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Draft reply with AI" }));

    expect(await screen.findByText("Can we meet Friday?")).toBeInTheDocument();
    expect(screen.getByText("Project timing")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Optional short instruction" }), {
      target: { value: "Accept and ask what time." },
    });
    fireEvent.click(screen.getByRole("button", { name: "Generate draft" }));

    await waitFor(() => expect(generate).toHaveBeenCalledWith(
      context,
      "Accept and ask what time.",
      "openai",
      "gpt-4o",
      null,
    ));
    const editor = screen.getByRole("textbox", { name: "Message body" });
    await waitFor(() => expect(editor).toHaveTextContent('<img src=x onerror="alert(1)">Friday works for me.'));
    expect(editor.querySelector("img")).toBeNull();
    expect(editor).toHaveTextContent("Can we meet Friday?");
  });

  it("requires confirmation before adding a suggestion above existing authored text", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const generate = vi.spyOn(mailClient, "generateReply").mockResolvedValue({ body: "Suggested reply." });
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const existing = { ...replyDraft, body: `My existing words.${replyDraft.body}` };

    render(<Composer draft={existing} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Draft reply with AI" }));
    await screen.findByText("Can we meet Friday?");
    fireEvent.click(screen.getByRole("button", { name: "Generate draft" }));

    expect(generate).not.toHaveBeenCalled();
    expect(screen.getByText(/already contains text/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add anyway" }));

    await waitFor(() => expect(generate).toHaveBeenCalledTimes(1));
    const editor = screen.getByRole("textbox", { name: "Message body" });
    await waitFor(() => expect(editor).toHaveTextContent("Suggested reply."));
    expect(editor).toHaveTextContent("My existing words.");
  });
});

describe("Composer recipient autocomplete", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  const contact = {
    email: "jane@example.com",
    displayName: "Jane Doe",
    sentCount: 4,
    receivedCount: 1,
    lastInteractedAt: "2026-03-05T00:00:00Z",
    pinned: false,
  };

  it("suggests a past correspondent from local history and fills the field on selection", async () => {
    const suggest = vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([contact]);
    render(<Composer draft={draft} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });

    fireEvent.change(to, { target: { value: "ja" } });
    await vi.advanceTimersByTimeAsync(150);
    expect(suggest).toHaveBeenCalledWith("first@example.com", "ja", 8);

    const option = await screen.findByRole("option", { name: /Jane Doe/ });
    fireEvent.mouseDown(option);
    expect(screen.getByRole("button", { name: "Remove Jane Doe" })).toBeInTheDocument();
    expect(to).toHaveValue("");
  });

  it("pins a suggested contact without inserting it into the field", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([contact]);
    const pin = vi.spyOn(mailClient, "pinContact").mockResolvedValue();
    render(<Composer draft={draft} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });

    fireEvent.change(to, { target: { value: "ja" } });
    await vi.advanceTimersByTimeAsync(150);
    await screen.findByRole("option", { name: /Jane Doe/ });

    fireEvent.click(screen.getByRole("button", { name: "Pin jane@example.com" }));
    expect(pin).toHaveBeenCalledWith("first@example.com", "jane@example.com", "Jane Doe");
    expect(to).toHaveValue("ja");
  });

  it("offers to pin a brand-new address that has no mail history at all", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([]);
    const pin = vi.spyOn(mailClient, "pinContact").mockResolvedValue();
    render(<Composer draft={draft} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });

    fireEvent.change(to, { target: { value: "wife@example.com" } });
    await vi.advanceTimersByTimeAsync(150);

    const option = await screen.findByRole("option", { name: /Pin wife@example.com as a contact/ });
    fireEvent.mouseDown(option);

    expect(pin).toHaveBeenCalledWith("first@example.com", "wife@example.com", null);
    expect(screen.getByRole("button", { name: "Remove wife@example.com" })).toBeInTheDocument();
    expect(to).toHaveValue("");
  });

  it("shows a prefilled reply recipient as a badge immediately, with no mail history query needed", () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([]);
    const prefilled = { ...draft, to: "hello@dispatch.local" };
    render(<Composer draft={prefilled} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);

    expect(screen.getByRole("button", { name: "Remove hello@dispatch.local" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "To" })).toHaveValue("");
  });

  it("removes a recipient badge via its remove button and via Backspace on an empty field", () => {
    const prefilled = { ...draft, to: "a@example.com, b@example.com" };
    render(<Composer draft={prefilled} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);

    fireEvent.click(screen.getByRole("button", { name: "Remove a@example.com" }));
    expect(screen.queryByRole("button", { name: "Remove a@example.com" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove b@example.com" })).toBeInTheDocument();

    fireEvent.keyDown(screen.getByRole("textbox", { name: "To" }), { key: "Backspace" });
    expect(screen.queryByRole("button", { name: "Remove b@example.com" })).not.toBeInTheDocument();
  });

  it("removes a newly added recipient without activating the first recipient's remove button", () => {
    const prefilled = { ...draft, to: "original@example.com" };
    render(<Composer draft={prefilled} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });

    fireEvent.change(to, { target: { value: "added@example.com" } });
    fireEvent.keyDown(to, { key: "Enter" });

    // The chip buttons must not be descendants of the input's label. Native
    // label activation would otherwise forward a click to the first button.
    expect(to.closest("label")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Remove added@example.com" }));

    expect(screen.getByRole("button", { name: "Remove original@example.com" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove added@example.com" })).not.toBeInTheDocument();
  });

  it("drags a recipient badge from To into Cc, moving it rather than copying it", () => {
    const prefilled = { ...draft, to: "hello@dispatch.local", cc: "" };
    render(<Composer draft={prefilled} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Cc / Bcc" }));

    const chip = screen.getByRole("button", { name: "Remove hello@dispatch.local" }).closest(".recipient-chip");
    const ccRow = screen.getByRole("textbox", { name: "Cc" }).closest(".recipient-chip-row");
    expect(chip).toBeTruthy();
    expect(ccRow).toBeTruthy();

    const store = new Map<string, string>();
    const dataTransfer = {
      setData: (type: string, val: string) => store.set(type, val),
      getData: (type: string) => store.get(type) ?? "",
      dropEffect: "move",
      effectAllowed: "move",
    };

    fireEvent.dragStart(chip!, { dataTransfer });
    fireEvent.dragOver(ccRow!, { dataTransfer });
    fireEvent.drop(ccRow!, { dataTransfer });
    fireEvent.dragEnd(chip!, { dataTransfer });

    const toField = screen.getByRole("textbox", { name: "To" }).closest(".compose-field") as HTMLElement;
    const ccField = screen.getByRole("textbox", { name: "Cc" }).closest(".compose-field") as HTMLElement;
    expect(within(toField).queryByRole("button", { name: "Remove hello@dispatch.local" })).not.toBeInTheDocument();
    expect(within(ccField).getByRole("button", { name: "Remove hello@dispatch.local" })).toBeInTheDocument();
  });
});
