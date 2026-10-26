import { createRef } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Composer, type ComposerHandle } from "./Composer";
import { DraftReviewSection, type DraftReviewAvailability } from "./DraftReviewSection";
import type { Draft } from "./correspondence";
import type { Account, DraftReviewResult } from "./domain";
import type { DraftReviewActions } from "./draftReview";
import { mailClient } from "./data/client";
import { saveAiProvider } from "./aiSettings";
import { resetWritingProfilesForTests, setWritingProfile } from "./writingProfiles";

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

/** Renders a composer; Draft Review opens, and so requests a review, with `open`. */
function setup(initial = draft, available = true, { open = true } = {}) {
  const ref = createRef<ComposerHandle>();
  const actions: DraftReviewActions = {
    readDraft: () => ref.current!.draftReview.readDraft(),
    replaceDraft: (expected, edit) => ref.current!.draftReview.replaceDraft(expected, edit),
  };
  const onOpenSettings = vi.fn();
  const onOpenWritingProfile = vi.fn();
  const tree = (withReview: boolean) => <>
    <Composer ref={ref} draft={initial} accounts={[{ email: initial.account }] as Account[]} snippets={[]}
      onCreateSnippet={vi.fn()} onUpdateSnippet={vi.fn()} onDeleteSnippet={vi.fn()} onClose={vi.fn()} onQueued={vi.fn()} />
    {withReview ? <DraftReviewSection actions={actions} account={initial.account} enabled available={available} onOpenSettings={onOpenSettings} onOpenWritingProfile={onOpenWritingProfile} /> : null}
  </>;
  const view = render(tree(open));
  return {
    ...view, ref, actions, onOpenSettings, onOpenWritingProfile,
    body: screen.getByRole("textbox", { name: "Message Body" }),
    openReview: () => view.rerender(tree(true)),
  };
}

async function reviewed() {
  await screen.findByText(result.assessment);
}

function whatWasSent() {
  return screen.getByText(/^What was sent to/).closest("details")!;
}

describe("Draft review", () => {
  beforeEach(() => {
    localStorage.clear();
    resetWritingProfilesForTests();
    saveAiProvider("openai");
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (value) => ({ ...value, revision: value.revision + 1 }));
    vi.spyOn(mailClient, "reviewDraft").mockResolvedValue(result);
    vi.spyOn(mailClient, "queueDraft");
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

  it("reviews the latest unsaved written text as soon as it opens, and again with a goal", async () => {
    const { body, openReview } = setup(draft, true, { open: false });
    body.innerHTML = "<p>A fresh edit.</p>";
    fireEvent.input(body);
    openReview();
    expect(screen.getByText("Reviewing…")).toHaveAttribute("role", "status");
    await reviewed();
    expect(mailClient.reviewDraft).toHaveBeenCalledWith({ subject: "Hello", body: "A fresh edit.", goal: "", aboutYou: "", reviewInstructions: "" }, "openai", expect.any(String), null);
    expect(within(whatWasSent()).getByText("A fresh edit.", { selector: "pre" })).toBeInTheDocument();
    expect(whatWasSent()).not.toHaveAttribute("open");

    fireEvent.change(screen.getByRole("textbox", { name: /Add a goal for this email/ }), { target: { value: " Get an introductory call " } });
    fireEvent.click(screen.getByRole("button", { name: "Review Again" }));
    await reviewed();
    expect(mailClient.reviewDraft).toHaveBeenLastCalledWith(expect.objectContaining({ body: "A fresh edit.", goal: "Get an introductory call" }), "openai", expect.any(String), null);
    expect(within(whatWasSent()).getByText("Get an introductory call")).toBeInTheDocument();
    expect(body).toHaveTextContent("A fresh edit.");
    expect(mailClient.queueDraft).not.toHaveBeenCalled();
  });

  it("reviews again from Enter in the goal field", async () => {
    setup();
    await reviewed();
    const goal = screen.getByRole("textbox", { name: /Add a goal for this email/ });
    fireEvent.change(goal, { target: { value: "Close the deal" } });
    fireEvent.keyDown(goal, { key: "Enter" });
    await waitFor(() => expect(mailClient.reviewDraft).toHaveBeenCalledTimes(2));
    expect(mailClient.reviewDraft).toHaveBeenLastCalledWith(expect.objectContaining({ goal: "Close the deal" }), "openai", expect.any(String), null);
  });

  it("sends the sending account's writing profile and shows it under What Was Sent", async () => {
    setWritingProfile("Me@Example.com", { aboutYou: "CEO of a consulting firm.", reviewInstructions: "Push for one next step." });
    setWritingProfile("other@example.com", { aboutYou: "Other account", reviewInstructions: "" });
    setup();
    await reviewed();
    expect(mailClient.reviewDraft).toHaveBeenCalledWith({
      subject: "Hello", body: "Let me know your thoughts.", goal: "",
      aboutYou: "CEO of a consulting firm.", reviewInstructions: "Push for one next step.",
    }, "openai", expect.any(String), null);
    const sent = whatWasSent();
    expect(within(sent).getByRole("heading", { name: "Your writing profile" })).toBeInTheDocument();
    expect(within(sent).getByText("CEO of a consulting firm.")).toBeInTheDocument();
    expect(within(sent).getByText("Push for one next step.")).toBeInTheDocument();
    expect(within(sent).queryByText("Other account")).not.toBeInTheDocument();
    expect(within(sent).getByText(/writing profile, and goal shown here are sent/)).toBeInTheDocument();
    expect(screen.queryByText(/No writing profile/)).not.toBeInTheDocument();
  });

  it("opens Writing Profile settings when the sending account has no writing profile", async () => {
    setWritingProfile("other@example.com", { aboutYou: "Other account", reviewInstructions: "" });
    const { onOpenSettings, onOpenWritingProfile } = setup();
    await reviewed();
    expect(screen.getByText(/No writing profile for this account/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add Writing Profile" }));
    expect(onOpenWritingProfile).toHaveBeenCalledOnce();
    expect(onOpenSettings).not.toHaveBeenCalled();
    expect(within(whatWasSent()).queryByRole("heading", { name: "Your writing profile" })).not.toBeInTheDocument();
    expect(within(whatWasSent()).getByText(/written text and goal shown here are sent/)).toBeInTheDocument();
  });

  it("applies only on request, saves the revision, and restores original subject and formatting with undo", async () => {
    const { body, ref } = setup();
    await reviewed();
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

  it("reviews the current draft again after edits, and refuses to apply a review of older text", async () => {
    const { body } = setup();
    await reviewed();
    body.innerHTML = "Newer words";
    fireEvent.input(body);
    fireEvent.click(screen.getByRole("button", { name: "Apply Revision" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Your draft changed");
    expect(body).toHaveTextContent("Newer words");
    fireEvent.click(screen.getByRole("button", { name: "Review Again" }));
    await reviewed();
    expect(mailClient.reviewDraft).toHaveBeenLastCalledWith(expect.objectContaining({ body: "Newer words" }), "openai", expect.any(String), null);
    fireEvent.click(screen.getByRole("button", { name: "Apply Revision" }));
    expect(body).toHaveTextContent(result.revisedBody);
  });

  it("protects edits made while feedback is pending from replacement", async () => {
    let resolve!: (value: DraftReviewResult) => void;
    vi.mocked(mailClient.reviewDraft).mockReturnValue(new Promise((yes) => { resolve = yes; }));
    const { body } = setup();
    body.innerHTML = "Keep these newer words";
    fireEvent.input(body);
    await act(async () => { resolve(result); });
    fireEvent.click(screen.getByRole("button", { name: "Apply Revision" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Your draft changed");
    expect(body).toHaveTextContent("Keep these newer words");
  });

  it("protects later edits from undo and guards recipient changes too", async () => {
    const { body } = setup();
    await reviewed();
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
    await reviewed();
    expect(mailClient.reviewDraft).toHaveBeenCalledWith(expect.objectContaining({ body: "Let me know your thoughts." }), "openai", expect.any(String), null);
    fireEvent.click(screen.getByRole("button", { name: "Apply Revision" }));
    const saved = await act(async () => ref.current!.flush());
    expect(saved.bodyHtml).toContain("On Monday, Client wrote:");
    expect(saved.bodyHtml).toContain("<p>Private quoted history</p></blockquote>");
    expect(saved.attachments).toEqual([attachment]);
    expect(mailClient.queueDraft).not.toHaveBeenCalled();
  });

  it("renders provider markup as text in feedback, the revision, and the applied body", async () => {
    const malicious = '<img src="https://evil.example/track" onerror="alert(1)">';
    vi.mocked(mailClient.reviewDraft).mockResolvedValue({ ...result, assessment: malicious, revisedBody: malicious });
    const { body, container } = setup();
    await screen.findByText(malicious, { selector: "pre" });
    expect(container.querySelector(".draft-review img")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Apply Revision" }));
    expect(body).toHaveTextContent(malicious);
    expect(body.querySelector("img")).toBeNull();
  });

  it("keeps a draft with inline images intact and offers feedback without replacing its layout", async () => {
    const { body } = setup({ ...draft, bodyHtml: `${draft.bodyHtml}<img src="cid:logo" alt="Logo">` });
    await reviewed();
    expect(screen.getByRole("button", { name: "Apply Revision" })).toBeDisabled();
    expect(body.querySelector("img")).toBeInTheDocument();
  });

  it("protects citations mixed into an inline reply from body replacement", async () => {
    const { body, ref } = setup({ ...draft, mode: "reply", bodyHtml: 'On Monday, Client wrote:<blockquote type="cite">Their words.</blockquote>My inline answer.' });
    await reviewed();
    expect(screen.getByRole("button", { name: "Apply Revision" })).toBeDisabled();
    expect(body.querySelector("blockquote")).toHaveTextContent("Their words.");
    const snapshot = ref.current!.draftReview.readDraft();
    expect(() => ref.current!.draftReview.replaceDraft(snapshot, { subject: "Changed", body: "New words" })).toThrow("preserve inline images and quoted text");
  });

  it("preserves formatting when only the subject changes, including during undo", async () => {
    vi.mocked(mailClient.reviewDraft).mockResolvedValue({ ...result, revisedBody: draft.body });
    const { body } = setup();
    await reviewed();
    fireEvent.click(screen.getByRole("button", { name: "Apply Revision" }));
    expect(body.querySelector("strong")).toHaveTextContent(draft.body);
    expect(screen.getByRole("textbox", { name: "Subject" })).toHaveValue(result.revisedSubject);
    fireEvent.click(screen.getByRole("button", { name: "Undo Revision" }));
    expect(body.querySelector("strong")).toHaveTextContent(draft.body);
    expect(screen.getByRole("textbox", { name: "Subject" })).toHaveValue(draft.subject);
  });

  it("supports a good draft with no suggested changes", async () => {
    vi.mocked(mailClient.reviewDraft).mockResolvedValue({ ...result, suggestions: [], revisedBody: draft.body, revisedSubject: draft.subject });
    setup();
    await reviewed();
    expect(screen.getByText("No changes suggested.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Apply Revision" })).not.toBeInTheDocument();
  });

  it("preserves the draft on provider failure and allows retry", async () => {
    vi.mocked(mailClient.reviewDraft).mockRejectedValueOnce(new Error("Provider unavailable"));
    const { body } = setup();
    expect(await screen.findByRole("alert")).toHaveTextContent("Provider unavailable");
    expect(body.querySelector("strong")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Review Again" }));
    await reviewed();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("is absent when Draft Assist is off, and marks Review Again as sending the draft", async () => {
    const actions: DraftReviewActions = { readDraft: vi.fn(), replaceDraft: vi.fn() };
    const { container } = render(<DraftReviewSection actions={actions} enabled={false} available={false} onOpenSettings={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
    expect(actions.readDraft).not.toHaveBeenCalled();
    cleanup();

    setup();
    await reviewed();
    expect(screen.getByRole("button", { name: "Review Again" }).querySelector("svg.lucide-sparkles")).not.toBeNull();
    expect(screen.getByRole("button", { name: "Apply Revision" }).querySelector("svg")).toBeNull();
  });

  it("guides setup when unavailable and rejects empty written text without sending", async () => {
    const { onOpenSettings, unmount } = setup(draft, false);
    fireEvent.click(screen.getByRole("button", { name: "AI Settings" }));
    expect(onOpenSettings).toHaveBeenCalledOnce();
    expect(mailClient.reviewDraft).not.toHaveBeenCalled();
    unmount();
    setup({ ...draft, body: "", bodyHtml: "" });
    expect(await screen.findByRole("alert")).toHaveTextContent("Write some email text");
    expect(mailClient.reviewDraft).not.toHaveBeenCalled();
    expect(screen.queryByText(/^What was sent to/)).not.toBeInTheDocument();
  });

  it("starts the review once the API key check confirms one can be sent", async () => {
    const { rerender } = render(<DraftReviewSection actions={{ readDraft: vi.fn(() => ({ id: "d", account: "me@example.com", subject: "", body: "Hi", bodyHtml: "<p>Hi</p>", fingerprint: "f", hasInlineImages: false, hasInlineQuotes: false })), replaceDraft: vi.fn() }} enabled available={false} />);
    expect(mailClient.reviewDraft).not.toHaveBeenCalled();
    const actions = { readDraft: vi.fn(() => ({ id: "d", account: "me@example.com", subject: "", body: "Hi", bodyHtml: "<p>Hi</p>", fingerprint: "f", hasInlineImages: false, hasInlineQuotes: false })), replaceDraft: vi.fn() };
    rerender(<DraftReviewSection actions={actions} enabled available />);
    await reviewed();
    rerender(<DraftReviewSection actions={actions} enabled available />);
    expect(mailClient.reviewDraft).toHaveBeenCalledOnce();
  });

  it("ignores feedback when the draft panel closes", async () => {
    let resolve!: (value: DraftReviewResult) => void;
    vi.mocked(mailClient.reviewDraft).mockReturnValue(new Promise((yes) => { resolve = yes; }));
    const { unmount } = setup();
    await waitFor(() => expect(mailClient.reviewDraft).toHaveBeenCalledOnce());
    unmount();
    await act(async () => { resolve(result); });
    expect(screen.queryByText(result.assessment)).not.toBeInTheDocument();
    expect(mailClient.saveDraft).not.toHaveBeenCalled();
  });
});

describe("Draft review in the composer", () => {
  const composer = (initial: Draft, review: DraftReviewAvailability) => (
    <Composer draft={initial} accounts={[{ email: initial.account }] as Account[]} snippets={[]}
      onCreateSnippet={vi.fn()} onUpdateSnippet={vi.fn()} onDeleteSnippet={vi.fn()} onClose={vi.fn()} onQueued={vi.fn()} review={review} />
  );
  const renderComposer = (initial: Draft = draft, review: DraftReviewAvailability = { enabled: true, available: true }) => render(composer(initial, review));
  beforeEach(() => {
    localStorage.clear();
    saveAiProvider("openai");
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (value) => ({ ...value, revision: value.revision + 1 }));
    vi.spyOn(mailClient, "reviewDraft").mockResolvedValue(result);
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

  it("opens below the message body from Review in the writing tools, and closes again", async () => {
    renderComposer();
    const tools = screen.getByRole("group", { name: "Writing tools" });
    const trigger = await within(tools).findByRole("button", { name: "Review" });
    expect(trigger.querySelector("svg.lucide-sparkles")).not.toBeNull();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("region", { name: "Draft Review" })).not.toBeInTheDocument();

    fireEvent.click(trigger);
    const review = await screen.findByRole("region", { name: "Draft Review" });
    const heading = within(review).getByRole("heading", { name: "Draft Review" });
    const glyph = heading.querySelector(".context-row-glyph");
    expect(glyph).toHaveAttribute("aria-hidden", "true");
    expect(glyph?.querySelector("svg.lucide-sparkles")).not.toBeNull();
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    // Opening it is the request; the draft stays editable above it.
    expect(screen.getByRole("textbox", { name: "Message Body" })).not.toHaveAttribute("contenteditable", "false");
    expect(await within(review).findByText(result.assessment)).toBeInTheDocument();
    expect(mailClient.reviewDraft).toHaveBeenCalledOnce();

    // A goal typed for this draft is still there after closing and reopening.
    fireEvent.change(within(review).getByRole("textbox", { name: /Add a goal for this email/ }), { target: { value: "Book a call" } });
    fireEvent.click(within(review).getByRole("button", { name: "Close Draft Review" }));
    fireEvent.click(trigger);
    const reopened = await screen.findByRole("region", { name: "Draft Review" });
    await waitFor(() => expect(mailClient.reviewDraft).toHaveBeenCalledTimes(2));
    expect(mailClient.reviewDraft).toHaveBeenLastCalledWith(expect.objectContaining({ goal: "Book a call" }), "openai", expect.any(String), null);
    expect(await within(reopened).findByRole("textbox", { name: /Add a goal for this email/ })).toHaveValue("Book a call");

    fireEvent.click(within(reopened).getByRole("button", { name: "Close Draft Review" }));
    expect(screen.queryByRole("region", { name: "Draft Review" })).not.toBeInTheDocument();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
  });

  it("is not offered for a forward, and follows Draft Assist being turned on or off while the draft is open", () => {
    renderComposer({ ...draft, mode: "forward" });
    expect(screen.queryByRole("button", { name: "Review" })).not.toBeInTheDocument();
    cleanup();

    const { rerender } = renderComposer(draft, { enabled: false, available: false });
    expect(screen.queryByRole("button", { name: "Review" })).not.toBeInTheDocument();
    rerender(composer(draft, { enabled: true, available: true }));
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(screen.getByRole("region", { name: "Draft Review" })).toBeInTheDocument();
    rerender(composer(draft, { enabled: false, available: false }));
    expect(screen.queryByRole("region", { name: "Draft Review" })).not.toBeInTheDocument();
  });

  it("links AI settings when no API key is configured", async () => {
    const onOpenSettings = vi.fn();
    renderComposer(draft, { enabled: true, available: false, onOpenSettings });
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    const review = await screen.findByRole("region", { name: "Draft Review" });
    expect(review).toHaveTextContent("Set up an AI provider and API key");
    fireEvent.click(within(review).getByRole("button", { name: "AI Settings" }));
    expect(onOpenSettings).toHaveBeenCalledOnce();
    expect(mailClient.reviewDraft).not.toHaveBeenCalled();
  });
});
