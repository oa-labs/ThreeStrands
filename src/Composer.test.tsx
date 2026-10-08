import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createRef } from "react";
import DOMPurify from "dompurify";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Composer, type ComposerHandle } from "./Composer";
import type { Draft } from "./correspondence";
import { mailClient } from "./data/client";
import type { Account, Snippet } from "./domain";
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
  provider: "gmail",
  sortOrder,
  connectedAt: "2026-01-01T00:00:00Z",
  lastSyncedAt: null,
}));

const snippetProps = {
  snippets: [] as Snippet[],
  onCreateSnippet: vi.fn(),
  onUpdateSnippet: vi.fn(),
  onDeleteSnippet: vi.fn(),
};

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

    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const selector = screen.getByRole("combobox", { name: "Send From" });

    expect(fireEvent.pointerDown(selector, { button: 0, pointerId: 1 })).toBe(true);
    fireEvent.change(selector, { target: { value: "second@example.com" } });

    await waitFor(() => expect(selector).toHaveValue("second@example.com"));
    expect(mailClient.setDraftAccount).toHaveBeenCalledWith("draft-1", "second@example.com");
  });

  it("keeps the From selector inside the composer's keyboard focus loop", () => {
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const selector = screen.getByRole("combobox", { name: "Send From" });

    selector.focus();
    fireEvent.keyDown(selector, { key: "Tab", shiftKey: true });

    expect(screen.getByRole("button", { name: "Discard Draft" })).toHaveFocus();
  });

  it("gives a new message a taller body than a reply", () => {
    const { unmount } = render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    expect(screen.getByRole("dialog", { name: "New Message" })).toHaveClass("composer-new");
    unmount();
    render(<Composer draft={{ ...draft, mode: "reply" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    expect(screen.getByRole("dialog", { name: "Reply Message" })).not.toHaveClass("composer-new");

    const css = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");
    expect(css).toContain(".composer-new .compose-body:not(.compose-quoted) { height: min(45vh + 90px, 600px, 100vh - 400px); }");
  });
});

describe("Composer recipient visibility and shortcuts", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("shows populated copy fields by default and toggles only blank fields from the To label", () => {
    render(<Composer draft={{ ...draft, cc: "copy@example.com" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    expect(screen.getByRole("textbox", { name: "Cc" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Bcc" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cc / Bcc" })).not.toBeInTheDocument();

    const toLabel = screen.getByRole("button", { name: "To" });
    expect(toLabel).toHaveAttribute("title", "Click to show Cc/Bcc");

    fireEvent.click(toLabel);
    expect(screen.getByRole("textbox", { name: "Bcc" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "To" })).toHaveAttribute("title", "Click to hide Cc/Bcc");

    fireEvent.click(screen.getByRole("button", { name: "To" }));
    expect(screen.getByRole("textbox", { name: "Cc" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Bcc" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "To" })).toHaveAttribute("title", "Click to show Cc/Bcc");
  });

  it.each([
    ["o", "To"],
    ["c", "Cc"],
    ["b", "Bcc"],
  ])("focuses the %s recipient using its shortcut", (key, label) => {
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    fireEvent.keyDown(screen.getByRole("dialog"), { key, metaKey: true, shiftKey: true });

    expect(screen.getByRole("textbox", { name: label })).toHaveFocus();
  });
});

describe("Composer list marker shortcuts", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it.each([
    ["*", "ul > li"],
    ["1.", "ol > li"],
  ])("starts a list when space follows %s", (marker, selector) => {
    render(<Composer draft={{ ...draft, body: marker }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    const text = editor.firstChild!;
    const range = document.createRange();
    range.setStart(text, marker.length);
    range.collapse(true);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);

    fireEvent.keyDown(editor, { key: " " });

    expect(editor.querySelector(selector)).toBeInTheDocument();
    expect(editor).not.toHaveTextContent(marker);
  });
});

describe("Composer pasted links", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  const renderWithSelection = (collapsed: boolean) => {
    render(<Composer draft={{ ...draft, body: "Read the docs" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    const text = editor.firstChild!;
    const range = document.createRange();
    range.setStart(text, 9);
    range.setEnd(text, collapsed ? 9 : 13);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
    const execute = vi.fn(() => true);
    Object.defineProperty(document, "execCommand", { configurable: true, value: execute });
    return { editor, execute };
  };

  it("links the selected text when a URL is pasted over it", () => {
    const { editor, execute } = renderWithSelection(false);

    fireEvent.paste(editor, { clipboardData: { files: [], getData: () => "https://example.com/docs" } });

    expect(execute).toHaveBeenCalledWith("createLink", false, "https://example.com/docs");
    expect(execute).not.toHaveBeenCalledWith("insertHTML", expect.anything(), expect.anything());
  });

  it("inserts a pasted URL as a link when nothing is selected", () => {
    const { editor, execute } = renderWithSelection(true);

    fireEvent.paste(editor, { clipboardData: { files: [], getData: () => "https://example.com/docs" } });

    expect(execute).toHaveBeenCalledWith("insertHTML", false, '<a href="https://example.com/docs">https://example.com/docs</a>');
  });

  it("replaces the selection with pasted text that is not a lone URL", () => {
    const { editor, execute } = renderWithSelection(false);

    fireEvent.paste(editor, { clipboardData: { files: [], getData: () => "see https://example.com/docs" } });

    expect(execute).not.toHaveBeenCalledWith("createLink", expect.anything(), expect.anything());
    expect(execute).toHaveBeenCalledWith("insertHTML", false, 'see <a href="https://example.com/docs">https://example.com/docs</a>');
  });
});

describe("Composer body input responsiveness", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("defers cloning and sanitizing the message body until the autosave boundary", async () => {
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    const cloneNode = vi.spyOn(editor, "cloneNode");

    for (const text of ["A", "A longer", "A longer message"]) {
      editor.innerHTML = text;
      fireEvent.input(editor);
    }

    expect(cloneNode).not.toHaveBeenCalled();
    expect(saveDraft).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(300);

    // One copy of the body per autosave: HTML and plain text share it.
    expect(cloneNode).toHaveBeenCalledTimes(1);
    expect(saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      body: "A longer message",
      bodyHtml: "A longer message",
    }));
  });

  it("does not re-sanitize the quoted thread when the composer re-renders during input", async () => {
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const quoted = Array.from({ length: 200 }, (_, index) => `> Quoted line ${index}`).join("\n");
    const reply: Draft = { ...draft, mode: "replyAll", to: "a@example.com", body: `\n\nOn Monday, A wrote:\n${quoted}` };
    const props = { draft: reply, accounts, ...snippetProps, onClose: () => {}, onQueued: () => {} };
    const { rerender } = render(<Composer {...props} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    expect(screen.getByLabelText("Quoted Text").textContent).toContain("Quoted line 199");
    const sanitize = vi.spyOn(DOMPurify, "sanitize");

    // The first input flips the status to "Unsaved changes", and a parent
    // re-render (e.g. App state) renders the composer again. Neither may do
    // work proportional to the quoted thread before the text is painted.
    editor.insertAdjacentText("afterbegin", "Dictated reply. ");
    fireEvent.input(editor);
    rerender(<Composer {...props} />);

    expect(screen.getByRole("status")).toHaveTextContent("Unsaved changes");
    expect(sanitize).not.toHaveBeenCalled();
    expect(editor.textContent).toMatch(/^Dictated reply\. /);
  });

  it("captures the latest body immediately when a send or close flushes the draft", async () => {
    const ref = createRef<ComposerHandle>();
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(<Composer ref={ref} draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });

    editor.innerHTML = "<strong>Send this text</strong>";
    fireEvent.input(editor);
    await ref.current?.flush();

    expect(saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      body: "Send this text",
      bodyHtml: "<strong>Send this text</strong>",
    }));
  });

  it("discards a completely empty draft when the composer closes", async () => {
    const onClose = vi.fn();
    const discardDraft = vi.spyOn(mailClient, "discardDraft").mockResolvedValue();
    const saveDraft = vi.spyOn(mailClient, "saveDraft");
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={onClose} onQueued={() => {}} />);

    fireEvent.click(screen.getByRole("button", { name: "Save and Close Draft" }));

    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(discardDraft).toHaveBeenCalledWith("draft-1");
    expect(saveDraft).not.toHaveBeenCalled();
  });

  it("keeps a draft with authored content when the composer closes", async () => {
    const onClose = vi.fn();
    const discardDraft = vi.spyOn(mailClient, "discardDraft").mockResolvedValue();
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={onClose} onQueued={() => {}} />);

    fireEvent.change(screen.getByRole("textbox", { name: "Subject" }), { target: { value: "Keep this draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Save and Close Draft" }));

    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(saveDraft).toHaveBeenCalledWith(expect.objectContaining({ subject: "Keep this draft" }));
    expect(discardDraft).not.toHaveBeenCalled();
  });
});

describe("Composer pasted images", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("inserts, resizes, and removes an image pasted into the message body", async () => {
    const inlineAttachment = { id: "inline-1", name: "screenshot.png", mime: "image/png", size: 4, ready: true, messageId: null, providerId: null, inline: true, contentId: "inline-1@threestrands.local" };
    const attachInline = vi.spyOn(mailClient, "attachInlineImage").mockResolvedValue({ ...draft, revision: 1, attachments: [inlineAttachment] });
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const removeAttachment = vi.spyOn(mailClient, "removeAttachment").mockResolvedValue({ ...draft, revision: 3 });
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
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
    const handle = screen.getByRole("slider", { name: "Resize Pasted Image" });
    fireEvent.pointerDown(handle, { clientX: 100 });
    fireEvent.pointerMove(window, { clientX: 160 });
    fireEvent.pointerUp(window);
    expect(image).toHaveAttribute("width", "380");
    await waitFor(() => expect(saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      bodyHtml: expect.stringContaining('src="cid:inline-1@threestrands.local"'),
    })));

    fireEvent.click(screen.getByRole("button", { name: "Remove Pasted Image" }));
    expect(editor.querySelector("img")).not.toBeInTheDocument();
    await waitFor(() => expect(removeAttachment).toHaveBeenCalledWith("draft-1", "inline-1"));
  });

  it("restores an inline image preview when a saved draft is reopened", async () => {
    const savedDraft = {
      ...draft,
      bodyHtml: '<p>See below</p><img src="cid:inline-1@threestrands.local" alt="Screenshot" width="320">',
      attachments: [{ id: "inline-1", name: "screenshot.png", mime: "image/png", size: 4, ready: true, messageId: null, providerId: null, inline: true, contentId: "inline-1@threestrands.local" }],
    };
    const readInline = vi.spyOn(mailClient, "readInlineImage").mockResolvedValue("data:image/png;base64,iVBORw==");

    render(<Composer draft={savedDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    const image = screen.getByRole("img", { name: "Screenshot" });
    await waitFor(() => expect(image).toHaveAttribute("src", "data:image/png;base64,iVBORw=="));
    expect(readInline).toHaveBeenCalledWith("draft-1", "inline-1");
    expect(screen.getByRole("button", { name: "Remove Pasted Image" })).toBeInTheDocument();
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

    render(<Composer draft={forwarded} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={onQueued} />);

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

    render(<Composer draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: /Draft Reply With AI/ }));

    expect(await screen.findByText("Can we meet Friday?")).toBeInTheDocument();
    expect(screen.getByText("Project timing")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Optional Short Instruction" }), {
      target: { value: "Accept and ask what time." },
    });
    fireEvent.click(screen.getByRole("button", { name: "Generate Draft" }));

    await waitFor(() => expect(generate).toHaveBeenCalledWith(
      context,
      "Accept and ask what time.",
      "openai",
      "gpt-4o",
      null,
      "default",
    ));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Reply Assist" })).not.toBeInTheDocument());
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    await waitFor(() => expect(editor).toHaveTextContent('<img src=x onerror="alert(1)">Friday works for me.'));
    expect(editor.querySelector("img")).toBeNull();
    expect(screen.getByLabelText("Quoted Text")).toHaveTextContent("Can we meet Friday?");
  });

  it("opens a reply with the quoted source as a citation and saves a quoted text alternative", async () => {
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(<Composer draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    const editor = screen.getByRole("textbox", { name: "Message Body" });
    const quoted = screen.getByLabelText("Quoted Text");
    const citation = quoted.querySelector('blockquote[type="cite"]');
    expect(citation).toHaveTextContent("Can we meet Friday?");
    expect(citation?.textContent).not.toContain(">");

    editor.insertBefore(document.createTextNode("Friday works."), editor.firstChild);
    fireEvent.input(editor);

    await waitFor(() => expect(saveDraft).toHaveBeenCalled());
    const saved = saveDraft.mock.calls.at(-1)![0];
    expect(saved.bodyHtml).toContain('<blockquote type="cite"');
    expect(saved.body).toBe("Friday works.\n\nOn Sep 16, Sender wrote:\n> Can we meet Friday?");
  });

  it("inserts selected availability text above the editable quoted reply", async () => {
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(
      <Composer
        draft={replyDraft}
        accounts={accounts} {...snippetProps}
        onClose={() => {}}
        onQueued={() => {}}
        availabilityText={'Here are some times that work for me:\n\n- Tuesday, September 22, 2026 · 9:00–9:30 AM EDT (America/New_York)'}
      />,
    );

    const editor = screen.getByRole("textbox", { name: "Message Body" });
    await waitFor(() => expect(editor).toHaveTextContent("September 22, 2026"));
    expect(editor).not.toHaveTextContent("Can we meet Friday?");
    expect(screen.getByLabelText("Quoted Text")).toHaveTextContent("Can we meet Friday?");
  });

  it("shows a task-derived instruction in Reply Assist without generating automatically", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const generate = vi.spyOn(mailClient, "generateReply");
    render(
      <Composer
        draft={replyDraft}
        accounts={accounts} {...snippetProps}
        onClose={() => {}}
        onQueued={() => {}}
        replyAssistInstruction="Draft a concise follow-up about the launch date."
      />,
    );

    const assist = await screen.findByRole("dialog", { name: "Reply Assist" });
    expect(within(assist).getByText(/Task-derived instruction:/)).toBeInTheDocument();
    expect(generate).not.toHaveBeenCalled();
  });

  it("preserves the editable draft when Reply Assist fails", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    vi.spyOn(mailClient, "generateReply").mockRejectedValue(new Error("Provider unavailable"));
    render(
      <Composer
        draft={replyDraft}
        accounts={accounts} {...snippetProps}
        onClose={() => {}}
        onQueued={() => {}}
        replyAssistInstruction="Draft a concise follow-up."
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Generate Draft" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Provider unavailable");
    expect(screen.getByLabelText("Quoted Text")).toHaveTextContent("Can we meet Friday?");
  });

  it("requires confirmation before adding a suggestion above existing authored text", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const generate = vi.spyOn(mailClient, "generateReply").mockResolvedValue({ body: "Suggested reply." });
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const existing = { ...replyDraft, body: `My existing words.${replyDraft.body}` };

    render(<Composer draft={existing} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: /Draft Reply With AI/ }));
    await screen.findByText("Can we meet Friday?");
    fireEvent.click(screen.getByRole("button", { name: "Generate Draft" }));

    expect(generate).not.toHaveBeenCalled();
    expect(screen.getByText(/already contains text/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add Anyway" }));

    await waitFor(() => expect(generate).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Reply Assist" })).not.toBeInTheDocument());
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    await waitFor(() => expect(editor).toHaveTextContent("Suggested reply."));
    expect(editor).toHaveTextContent("My existing words.");
  });

  it("opens Reply Assist through the exposed handle, the same way the Mod+J shortcut does", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const ref = createRef<ComposerHandle>();

    render(<Composer ref={ref} draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    await screen.findByRole("button", { name: /Draft Reply With AI/ });

    ref.current?.draftReplyWithAI();

    expect(await screen.findByText("Can we meet Friday?")).toBeInTheDocument();
  });

  it("opens over the composer, focuses the instruction, and restores focus when dismissed", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const { container } = render(<Composer draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const trigger = await screen.findByRole("button", { name: /Draft Reply With AI/ });
    trigger.focus();

    fireEvent.click(trigger);

    const assist = await screen.findByRole("dialog", { name: "Reply Assist" });
    expect(assist).toHaveAttribute("aria-modal", "true");
    expect(assist).toHaveClass("reply-assist-modal");
    expect(assist.parentElement).toHaveClass("reply-assist-backdrop");
    expect(assist.parentElement?.parentElement).toBe(document.body);
    expect(container).toHaveAttribute("aria-hidden", "true");
    expect(screen.getByRole("textbox", { name: "Optional Short Instruction" })).toHaveFocus();

    fireEvent.keyDown(window, { key: "Escape" });

    expect(screen.queryByRole("dialog", { name: "Reply Assist" })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Reply Message" })).toBeInTheDocument();
    expect(trigger).toHaveFocus();
    expect(container).not.toHaveAttribute("aria-hidden");
  });

  it("keeps a dismissed task-opened assist closed until the user opens it again", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} replyAssistInstruction="Ask for available times." />);

    const assist = await screen.findByRole("dialog", { name: "Reply Assist" });
    fireEvent.click(within(assist).getByRole("button", { name: "Close" }));

    expect(screen.queryByRole("dialog", { name: "Reply Assist" })).not.toBeInTheDocument();
    ref.current?.draftReplyWithAI();
    expect(await screen.findByRole("dialog", { name: "Reply Assist" })).toBeInTheDocument();
  });

  it("does nothing when Reply Assist is unavailable", async () => {
    saveAiProvider("none");
    const replyAssistContext = vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const ref = createRef<ComposerHandle>();

    render(<Composer ref={ref} draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    ref.current?.draftReplyWithAI();

    expect(screen.queryByText("Reply Assist")).not.toBeInTheDocument();
    expect(replyAssistContext).not.toHaveBeenCalled();
  });

  it("does not reopen or reload Reply Assist when it is already open", async () => {
    const replyAssistContext = vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    await screen.findByRole("button", { name: /Draft Reply With AI/ });

    act(() => ref.current?.draftReplyWithAI());
    const assist = await screen.findByRole("dialog", { name: "Reply Assist" });
    expect(await within(assist).findByText("Can we meet Friday?")).toBeInTheDocument();
    fireEvent.change(within(assist).getByRole("textbox", { name: "Optional Short Instruction" }), { target: { value: "Keep it short" } });

    act(() => ref.current?.draftReplyWithAI());

    expect(screen.getAllByRole("dialog", { name: "Reply Assist" })).toHaveLength(1);
    expect(within(assist).getByText("Can we meet Friday?")).toBeInTheDocument();
    expect(within(assist).getByRole("textbox", { name: "Optional Short Instruction" })).toHaveValue("Keep it short");
    expect(replyAssistContext).toHaveBeenCalledTimes(1);
  });
});

describe("Composer quoted history", () => {
  const reply: Draft = {
    ...draft,
    mode: "reply",
    sourceId: "source-message",
    to: "sender@example.com",
    subject: "Project timing",
    body: "\n\nOn Sep 16, Sender wrote:\n> Can we meet Friday?",
  };

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("keeps a reply's quoted history out of the body editor, collapsed until shown", () => {
    render(<Composer draft={reply} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const body = screen.getByRole("textbox", { name: "Message Body" });
    const quoted = screen.getByLabelText("Quoted Text");
    const toggle = screen.getByRole("button", { name: "Show Quoted Text" });

    expect(body).toBeEmptyDOMElement();
    expect(body).toHaveFocus();
    expect(quoted).not.toBeVisible();
    expect(quoted.querySelector('blockquote[type="cite"]')).toHaveTextContent("Can we meet Friday?");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveAttribute("aria-controls", quoted.id);

    fireEvent.click(toggle);
    expect(quoted).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Quoted Text" })).toBe(quoted);
    expect(screen.getByRole("button", { name: "Hide Quoted Text" })).toHaveAttribute("aria-expanded", "true");

    fireEvent.click(screen.getByRole("button", { name: "Hide Quoted Text" }));
    expect(quoted).not.toBeVisible();
  });

  it("saves edits to the shown quoted history after the authored text", async () => {
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(<Composer draft={reply} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const body = screen.getByRole("textbox", { name: "Message Body" });
    fireEvent.click(screen.getByRole("button", { name: "Show Quoted Text" }));
    const quoted = screen.getByRole("textbox", { name: "Quoted Text" });

    body.textContent = "Friday works.";
    fireEvent.input(body);
    quoted.querySelector("blockquote")!.textContent = "Can we meet Saturday?";
    fireEvent.input(quoted);

    await waitFor(() => expect(saveDraft).toHaveBeenCalled());
    const saved = saveDraft.mock.calls.at(-1)![0];
    expect(saved.body).toBe("Friday works.\n\nOn Sep 16, Sender wrote:\n> Can we meet Saturday?");
    expect(saved.bodyHtml).toMatch(/^Friday works\.<br><br>On Sep 16, Sender wrote:<blockquote type="cite"/);
  });

  it("splits a reopened reply draft's saved HTML back into authored text and collapsed history", async () => {
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const reopened: Draft = {
      ...reply,
      body: "Friday works.\n\nOn Sep 16, Sender wrote:\n> Can we meet Friday?",
      bodyHtml: 'Friday <b>works</b>.<br><br>On Sep 16, Sender wrote:<blockquote type="cite">Can we meet Friday?</blockquote>',
    };
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={reopened} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    expect(screen.getByRole("textbox", { name: "Message Body" })).toHaveTextContent(/^Friday works\.$/);
    expect(screen.getByLabelText("Quoted Text")).not.toBeVisible();

    fireEvent.input(screen.getByRole("textbox", { name: "Message Body" }));
    await ref.current?.flush();
    expect(saveDraft.mock.calls.at(-1)![0]).toEqual(expect.objectContaining({ body: reopened.body }));
    expect(saveDraft.mock.calls.at(-1)![0].bodyHtml).toMatch(/^Friday <b>works<\/b>\.<br><br>On Sep 16, Sender wrote:<blockquote type="cite"/);
  });

  it("leaves new messages and inline answers in a single body editor", () => {
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    expect(screen.queryByRole("button", { name: "Show Quoted Text" })).toBeNull();
    cleanup();

    const inline: Draft = { ...reply, bodyHtml: 'On Sep 16, Sender wrote:<blockquote type="cite">Can we meet Friday?</blockquote>Yes, Friday.' };
    render(<Composer draft={inline} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    expect(screen.queryByRole("button", { name: "Show Quoted Text" })).toBeNull();
    expect(screen.getByRole("textbox", { name: "Message Body" })).toHaveTextContent("Can we meet Friday?");
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
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });

    fireEvent.change(to, { target: { value: "ja" } });
    await vi.advanceTimersByTimeAsync(150);
    expect(suggest).toHaveBeenCalledWith("first@example.com", "ja", 8);

    const option = await screen.findByRole("option", { name: /Jane Doe/ });
    fireEvent.mouseDown(option);
    expect(screen.getByRole("button", { name: "Remove Jane Doe" })).toBeInTheDocument();
    expect(to).toHaveValue("");
  });

  const board = {
    id: "group-board",
    name: "Board",
    members: [
      { contactId: "contact:ada", displayName: "Ada Park", email: "ada@home.example", addresses: ["ada@home.example", "ada@work.example"] },
      { contactId: "contact:jane", displayName: "Jane Doe", email: "jane@example.com", addresses: ["jane@example.com"] },
      { contactId: "contact:sam", displayName: null, email: "sam@example.com", addresses: ["sam@example.com"] },
    ],
  };

  it("offers a matching group first and adds each member's primary address as a chip", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([contact]);
    vi.spyOn(mailClient, "listContactGroupRecipients").mockResolvedValue([board, { id: "empty", name: "Boardgames", members: [] }]);
    const onDraftChange = vi.fn();
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    render(<Composer draft={{ ...draft, to: "Jane Doe <jane@example.com>, " }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} onDraftChange={onDraftChange} />);
    const to = screen.getByRole("textbox", { name: "To" });
    fireEvent.focus(to);
    fireEvent.change(to, { target: { value: "boa" } });
    await vi.advanceTimersByTimeAsync(150);

    const options = within(await screen.findByRole("listbox", { name: "To suggestions" })).getAllByRole("option");
    // A group with no members is not offered.
    expect(options[0]).toHaveAccessibleName("Board, group of 3 members");
    expect(screen.queryByRole("option", { name: /Boardgames/ })).not.toBeInTheDocument();
    expect(to).toHaveAttribute("aria-activedescendant", "to-group-0");
    fireEvent.keyDown(to, { key: "Enter" });

    expect(screen.getByRole("button", { name: "Remove Ada Park" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove sam@example.com" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Remove Jane Doe" })).toHaveLength(1);
    expect(screen.getByText(/^Added 2 people/)).toHaveTextContent("Added 2 people from Board · 1 already in To");
    expect(screen.getByText(/^Added 2 people/)).toHaveAttribute("role", "status");
    // The note sits under the field's row, not inside it, so it can't push the chips off the label's line.
    expect(screen.getByText(/^Added 2 people/).closest(".recipient-field")).toBeNull();
    expect(to.closest(".recipient-field")?.nextElementSibling).toBe(screen.getByText(/^Added 2 people/));
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ to: "Jane Doe <jane@example.com>, Ada Park <ada@home.example>, sam@example.com, " }));
    fireEvent.change(to, { target: { value: "x" } });
    expect(screen.queryByText(/^Added 2 people/)).not.toBeInTheDocument();
  });

  it("quotes a group member whose name holds a comma, so the draft saves as one recipient", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([]);
    vi.spyOn(mailClient, "listContactGroupRecipients").mockResolvedValue([{ id: "eo", name: "EO", members: [
      { contactId: "contact:justin", displayName: "Fischgrund, Justin", email: "justin@example.com", addresses: ["justin@example.com"] },
      { contactId: "contact:kelly", displayName: "Kelly Sjol", email: "kelly@example.com", addresses: ["kelly@example.com"] },
    ] }]);
    const save = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });
    fireEvent.focus(to);
    fireEvent.change(to, { target: { value: "eo" } });
    await vi.advanceTimersByTimeAsync(150);
    fireEvent.mouseDown(await screen.findByRole("option", { name: /EO, group/ }));
    expect(screen.getByRole("button", { name: "Remove Fischgrund, Justin" })).toBeInTheDocument();
    await act(async () => { await ref.current!.flush(); });
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ to: '"Fischgrund, Justin" <justin@example.com>, Kelly Sjol <kelly@example.com>, ' }));
  });

  it("reads an older draft's unquoted comma as one recipient and saves it quoted", async () => {
    const save = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={{ ...draft, to: "Fischgrund, Justin <justin@example.com>, Kelly Sjol <kelly@example.com>, " }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    expect(screen.getByRole("button", { name: "Remove Fischgrund, Justin" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove Fischgrund" })).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Subject" }), { target: { value: "EO All Chapter Event" } });
    await act(async () => { await ref.current!.flush(); });
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ to: '"Fischgrund, Justin" <justin@example.com>, Kelly Sjol <kelly@example.com>, ' }));
  });

  it("moves through group and contact suggestions with the arrow keys", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([{ ...contact, displayName: "Board Chair", email: "chair@example.com" }]);
    vi.spyOn(mailClient, "listContactGroupRecipients").mockResolvedValue([board]);
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });
    fireEvent.focus(to);
    fireEvent.change(to, { target: { value: "board" } });
    await vi.advanceTimersByTimeAsync(150);
    await screen.findByRole("option", { name: /Board Chair/ });
    fireEvent.keyDown(to, { key: "ArrowDown" });
    expect(to).toHaveAttribute("aria-activedescendant", "to-suggestion-0");
    fireEvent.keyDown(to, { key: "Enter" });
    expect(screen.getByRole("button", { name: "Remove Board Chair" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove Ada Park" })).not.toBeInTheDocument();
  });

  it("pins a suggested contact without inserting it into the field", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([contact]);
    const pin = vi.spyOn(mailClient, "pinContact").mockResolvedValue();
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
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
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
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
    const prefilled = { ...draft, to: "hello@threestrands.local" };
    render(<Composer draft={prefilled} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    expect(screen.getByRole("button", { name: "Remove hello@threestrands.local" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "To" })).toHaveValue("");
  });

  it("removes a recipient badge via its remove button and via Backspace on an empty field", () => {
    const prefilled = { ...draft, to: "a@example.com, b@example.com" };
    render(<Composer draft={prefilled} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    fireEvent.click(screen.getByRole("button", { name: "Remove a@example.com" }));
    expect(screen.queryByRole("button", { name: "Remove a@example.com" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove b@example.com" })).toBeInTheDocument();

    fireEvent.keyDown(screen.getByRole("textbox", { name: "To" }), { key: "Backspace" });
    expect(screen.queryByRole("button", { name: "Remove b@example.com" })).not.toBeInTheDocument();
  });

  it("removes a newly added recipient without activating the first recipient's remove button", () => {
    const prefilled = { ...draft, to: "original@example.com" };
    render(<Composer draft={prefilled} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
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
    const prefilled = { ...draft, to: "hello@threestrands.local", cc: "" };
    render(<Composer draft={prefilled} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "To" }));

    const chip = screen.getByRole("button", { name: "Remove hello@threestrands.local" }).closest(".recipient-chip");
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
    expect(within(toField).queryByRole("button", { name: "Remove hello@threestrands.local" })).not.toBeInTheDocument();
    expect(within(ccField).getByRole("button", { name: "Remove hello@threestrands.local" })).toBeInTheDocument();
  });
});

describe("Composer context panel actions", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("reports recipient edits as they happen and body text at autosave", async () => {
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const onDraftChange = vi.fn();
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} onDraftChange={onDraftChange} />);
    fireEvent.change(screen.getByRole("textbox", { name: "Subject" }), { target: { value: "Lunch" } });
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ subject: "Lunch" }));

    const editor = screen.getByRole("textbox", { name: "Message Body" });
    editor.innerHTML = "See you there";
    fireEvent.input(editor);
    await waitFor(() => expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ body: "See you there" })));
  });

  it("moves the named addresses from To and Cc to Bcc", async () => {
    const onDraftChange = vi.fn();
    const ref = createRef<ComposerHandle>();
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    render(<Composer ref={ref} draft={{ ...draft, to: "Ann <ann@example.com>, bob@example.com, ", cc: "cy@example.com", bcc: "" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} onDraftChange={onDraftChange} />);
    act(() => ref.current!.moveRecipientsToBcc(["ANN@example.com", "cy@example.com"]));
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ to: "bob@example.com", cc: "", bcc: "Ann <ann@example.com>, cy@example.com" }));
    expect(screen.getByRole("textbox", { name: "Bcc" })).toBeInTheDocument();
  });

  it("swaps one recipient for another and leaves the rest", async () => {
    const onDraftChange = vi.fn();
    const ref = createRef<ComposerHandle>();
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    render(<Composer ref={ref} draft={{ ...draft, to: "Ann <ann@example.com>, jonh@example.com, " }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} onDraftChange={onDraftChange} />);
    act(() => ref.current!.replaceRecipient("JONH@example.com", "John <john@example.com>"));
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ to: "Ann <ann@example.com>, John <john@example.com>" }));
    expect(screen.getByText("John")).toBeInTheDocument();
  });

  it("returns focus and inserts text where the caret was before the body lost focus, else at the top", () => {
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={{ ...draft, body: "Hi Ann,\nBest, Me" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });

    act(() => ref.current!.insertText("Top line"));
    expect(editor.textContent?.startsWith("Top line")).toBe(true);

    // Put the caret after "Hi Ann," and leave the body.
    const walker = document.createTreeWalker(editor, NodeFilter.SHOW_TEXT);
    let greeting: Node | null = walker.nextNode();
    while (greeting && !greeting.textContent?.includes("Hi Ann,")) greeting = walker.nextNode();
    const caret = document.createRange();
    caret.setStart(greeting!, greeting!.textContent!.indexOf("Hi Ann,") + "Hi Ann,".length);
    caret.collapse(true);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(caret);
    fireEvent.blur(editor);

    act(() => ref.current!.focusBody());
    expect(editor).toHaveFocus();
    expect(window.getSelection()!.getRangeAt(0).startContainer).toBe(greeting);
    expect(window.getSelection()!.getRangeAt(0).startOffset).toBe(greeting!.textContent!.indexOf("Hi Ann,") + "Hi Ann,".length);

    act(() => ref.current!.insertText("Here are some times"));
    const text = editor.textContent ?? "";
    expect(text.indexOf("Hi Ann,")).toBeLessThan(text.indexOf("Here are some times"));
    expect(text.indexOf("Here are some times")).toBeLessThan(text.indexOf("Best, Me"));
  });
});
