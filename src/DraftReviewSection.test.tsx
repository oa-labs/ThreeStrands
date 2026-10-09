import { createRef } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Composer, type ComposerHandle } from "./Composer";
import { DraftReviewSection } from "./DraftReviewSection";
import type { Draft } from "./correspondence";
import type { Account, DraftReviewResult } from "./domain";
import type { DraftReviewActions } from "./draftReview";
import { mailClient } from "./data/client";
import { saveAiProvider } from "./aiSettings";

const draft: Draft = {
  id: "review-draft", revision: 0, account: "me@example.com", mode: "new",
  sourceId: null, threadId: null, replyId: null, references: [], to: "client@example.com", cc: "", bcc: "",
  subject: "Hello", body: "Let me know your thoughts.", bodyHtml: "<p><strong>Let me know your thoughts.</strong></p>",
  attachments: [], updatedAt: 0,
};
const result: DraftReviewResult = {
  assessment: "Your purpose is clear; make the next step easier to answer.",
  suggestions: [{ title: "Ask a clear question", field: "body", excerpt: "Let me know your thoughts.",
    reason: "A direct question makes replying easier.", replacement: "Would you be open to a brief call?" }],
  revisedSubject: "A brief introduction", revisedBody: "Would you be open to a brief call?",
};

function setup(initial = draft, available = true) {
  const ref = createRef<ComposerHandle>();
  const actions: DraftReviewActions = {
    readDraft: () => ref.current!.draftReview.readDraft(),
    replaceDraft: (expected, edit) => ref.current!.draftReview.replaceDraft(expected, edit),
  };
  const onOpenSettings = vi.fn();
  const view = render(<>
    <Composer ref={ref} draft={initial} accounts={[{ email: initial.account }] as Account[]} snippets={[]}
      onCreateSnippet={vi.fn()} onUpdateSnippet={vi.fn()} onDeleteSnippet={vi.fn()} onClose={vi.fn()} onQueued={vi.fn()} />
    <DraftReviewSection actions={actions} available={available} onOpenSettings={onOpenSettings} />
  </>);
  return { ...view, ref, actions, onOpenSettings, body: screen.getByRole("textbox", { name: "Message Body" }) };
}

async function reviewDraft() {
  fireEvent.click(screen.getByRole("button", { name: "Review Draft" }));
  fireEvent.click(screen.getByRole("button", { name: "Get Feedback" }));
  await screen.findByText(result.assessment);
}

describe("Draft review", () => {
  beforeEach(() => {
    localStorage.clear();
    saveAiProvider("openai");
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (value) => ({ ...value, revision: value.revision + 1 }));
    vi.spyOn(mailClient, "reviewDraft").mockResolvedValue(result);
    vi.spyOn(mailClient, "queueDraft");
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

  it("previews the latest unsaved written text and optional goal before requesting feedback", async () => {
    const { body } = setup();
    body.innerHTML = "<p>A fresh edit.</p>";
    fireEvent.input(body);
    fireEvent.change(screen.getByRole("textbox", { name: /What do you want/ }), { target: { value: "Get an introductory call" } });
    fireEvent.click(screen.getByRole("button", { name: "Review Draft" }));
    expect(screen.getByText("A fresh edit.", { selector: "pre" })).toBeInTheDocument();
    expect(mailClient.reviewDraft).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Get Feedback" }));
    await screen.findByText(result.assessment);
    expect(mailClient.reviewDraft).toHaveBeenCalledWith({ subject: "Hello", body: "A fresh edit.", goal: "Get an introductory call" }, "openai", expect.any(String), null);
    expect(body).toHaveTextContent("A fresh edit.");
    expect(mailClient.queueDraft).not.toHaveBeenCalled();
  });

  it("applies only on request, saves the revision, and restores original subject and formatting with undo", async () => {
    const { body, ref } = setup();
    await reviewDraft();
    expect(body.querySelector("strong")).toHaveTextContent("Let me know your thoughts.");
    expect(screen.getByText(result.suggestions[0].reason)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Apply Revision" }));
    expect(body).toHaveTextContent(result.revisedBody);
    expect(screen.getByRole("textbox", { name: "Subject" })).toHaveValue(result.revisedSubject);
    await act(async () => { await ref.current!.flush(); });
    expect(mailClient.saveDraft).toHaveBeenLastCalledWith(expect.objectContaining({ subject: result.revisedSubject, body: result.revisedBody }));
    fireEvent.click(screen.getByRole("button", { name: "Undo Revision" }));
    expect(body.querySelector("strong")).toHaveTextContent("Let me know your thoughts.");
    expect(screen.getByRole("textbox", { name: "Subject" })).toHaveValue("Hello");
    await act(async () => { await ref.current!.flush(); });
    expect(mailClient.saveDraft).toHaveBeenLastCalledWith(expect.objectContaining({ subject: "Hello", bodyHtml: draft.bodyHtml }));
  });

  it("refuses to send an outdated preview and can refresh it", async () => {
    const { body } = setup();
    fireEvent.click(screen.getByRole("button", { name: "Review Draft" }));
    body.innerHTML = "Newer words";
    fireEvent.input(body);
    fireEvent.click(screen.getByRole("button", { name: "Get Feedback" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Refresh the preview");
    expect(mailClient.reviewDraft).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /Review Again/ }));
    fireEvent.click(screen.getByRole("button", { name: "Get Feedback" }));
    await screen.findByText(result.assessment);
    expect(mailClient.reviewDraft).toHaveBeenCalledWith(expect.objectContaining({ body: "Newer words" }), "openai", expect.any(String), null);
  });

  it("protects edits made while feedback is pending from replacement", async () => {
    let resolve!: (value: DraftReviewResult) => void;
    vi.mocked(mailClient.reviewDraft).mockReturnValue(new Promise((yes) => { resolve = yes; }));
    const { body } = setup();
    fireEvent.click(screen.getByRole("button", { name: "Review Draft" }));
    fireEvent.click(screen.getByRole("button", { name: "Get Feedback" }));
    body.innerHTML = "Keep these newer words";
    fireEvent.input(body);
    await act(async () => { resolve(result); });
    fireEvent.click(screen.getByRole("button", { name: "Apply Revision" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Your draft changed");
    expect(body).toHaveTextContent("Keep these newer words");
  });

  it("protects later edits from undo and guards recipient changes too", async () => {
    const { body } = setup();
    await reviewDraft();
    fireEvent.click(screen.getByRole("button", { name: "Apply Revision" }));
    fireEvent.change(screen.getByRole("textbox", { name: "To" }), { target: { value: "someone-else@example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Undo Revision" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Your draft changed");
    expect(body).toHaveTextContent(result.revisedBody);
  });

  it("excludes a reply's quoted history and leaves it and attachments intact when applying", async () => {
    const quote = 'On Monday, Client wrote:<blockquote type="cite"><p>Private quoted history</p></blockquote>';
    const attachment = { id: "file-1", name: "proposal.pdf", size: 100, ready: true, inline: false, mime: "application/pdf", messageId: null, providerId: null };
    const { ref } = setup({ ...draft, mode: "reply", bodyHtml: `${draft.bodyHtml}${quote}`, attachments: [attachment] });
    await reviewDraft();
    expect(mailClient.reviewDraft).toHaveBeenCalledWith(expect.objectContaining({ body: "Let me know your thoughts." }), "openai", expect.any(String), null);
    fireEvent.click(screen.getByRole("button", { name: "Apply Revision" }));
    const saved = await act(async () => ref.current!.flush());
    expect(saved.bodyHtml).toContain("On Monday, Client wrote:");
    expect(saved.bodyHtml).toContain("<p>Private quoted history</p></blockquote>");
    expect(saved.attachments).toEqual([attachment]);
    expect(mailClient.queueDraft).not.toHaveBeenCalled();
  });

  it("renders provider markup as text in feedback, preview, and the applied body", async () => {
    const malicious = '<img src="https://evil.example/track" onerror="alert(1)">';
    vi.mocked(mailClient.reviewDraft).mockResolvedValue({ ...result, assessment: malicious, revisedBody: malicious });
    const { body, container } = setup();
    fireEvent.click(screen.getByRole("button", { name: "Review Draft" }));
    fireEvent.click(screen.getByRole("button", { name: "Get Feedback" }));
    await screen.findByText(malicious, { selector: "pre" });
    expect(container.querySelector(".draft-review img")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Apply Revision" }));
    expect(body).toHaveTextContent(malicious);
    expect(body.querySelector("img")).toBeNull();
  });

  it("keeps a draft with inline images intact and offers feedback without replacing its layout", async () => {
    const { body } = setup({ ...draft, bodyHtml: `${draft.bodyHtml}<img src="cid:logo" alt="Logo">` });
    await reviewDraft();
    expect(screen.getByRole("button", { name: "Apply Revision" })).toBeDisabled();
    expect(body.querySelector("img")).toBeInTheDocument();
  });

  it("supports a good draft with no suggested changes", async () => {
    vi.mocked(mailClient.reviewDraft).mockResolvedValue({ ...result, suggestions: [], revisedBody: draft.body, revisedSubject: draft.subject });
    setup();
    await reviewDraft();
    expect(screen.getByText("No changes suggested.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Apply Revision" })).not.toBeInTheDocument();
  });

  it("preserves the draft on provider failure and allows retry", async () => {
    vi.mocked(mailClient.reviewDraft).mockRejectedValueOnce(new Error("Provider unavailable"));
    const { body } = setup();
    fireEvent.click(screen.getByRole("button", { name: "Review Draft" }));
    fireEvent.click(screen.getByRole("button", { name: "Get Feedback" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Provider unavailable");
    expect(body.querySelector("strong")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Get Feedback" }));
    await screen.findByText(result.assessment);
  });

  it("guides setup when unavailable and rejects empty written text", async () => {
    const { onOpenSettings, unmount } = setup(draft, false);
    fireEvent.click(screen.getByRole("button", { name: "AI Settings" }));
    expect(onOpenSettings).toHaveBeenCalledOnce();
    expect(mailClient.reviewDraft).not.toHaveBeenCalled();
    unmount();
    setup({ ...draft, body: "", bodyHtml: "" });
    fireEvent.click(screen.getByRole("button", { name: "Review Draft" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Write some email text");
  });

  it("ignores feedback when the draft panel closes", async () => {
    let resolve!: (value: DraftReviewResult) => void;
    vi.mocked(mailClient.reviewDraft).mockReturnValue(new Promise((yes) => { resolve = yes; }));
    const { unmount } = setup();
    fireEvent.click(screen.getByRole("button", { name: "Review Draft" }));
    fireEvent.click(screen.getByRole("button", { name: "Get Feedback" }));
    await waitFor(() => expect(mailClient.reviewDraft).toHaveBeenCalledOnce());
    unmount();
    await act(async () => { resolve(result); });
    expect(screen.queryByText(result.assessment)).not.toBeInTheDocument();
    expect(mailClient.saveDraft).not.toHaveBeenCalled();
  });
});
