import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createRef } from "react";
import { MarkdownEditor, type EditorHandle } from "@meowdown/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NoteLinkAutocomplete, NOTE_LINK_TRIGGER } from "./NoteLinkAutocomplete";
import { markdownEntityLink } from "./noteEntities";
import type { NoteLinkChoice } from "./NoteLinkPicker";

const contact: NoteLinkChoice = { label: "Ada [Work]", detail: "ada@example.com", reference: { version: 1, kind: "contact", id: "ada@example.com", contact: "c-ada" } };
const conversation: NoteLinkChoice = { label: "Project review", detail: "Ada · me@example.com", sentAt: "2026-11-10T15:00:00Z", reference: { version: 1, kind: "conversation", id: "thread-review", account: "me@example.com" } };

async function mount({ initial = "Discuss ", onSearchNotes = vi.fn(async (_query: string) => [{ target: "Ada's notes" }]), onSearchEntities = vi.fn(async (_query: string): Promise<NoteLinkChoice[]> => [contact, conversation]) } = {}) {
  const ref = createRef<EditorHandle>();
  render(<MarkdownEditor handleRef={ref} initialMarkdown={initial} embedPaste={false} caretGlide={false} blockHandle={false}>
    <NoteLinkAutocomplete onSearchNotes={onSearchNotes} onSearchEntities={onSearchEntities} />
  </MarkdownEditor>);
  await waitFor(() => expect(ref.current?.getEditor()?.mounted).toBe(true));
  // Let the custom elements register their editor extensions before typing.
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
  act(() => { ref.current!.setSelection("end"); ref.current!.focus(); });
  const type = async (text: string) => {
    await act(async () => {
      const view = ref.current!.getEditor()!.view;
      const { from, to } = view.state.selection;
      const handled = view.someProp("handleTextInput", (handler) => handler(view, from, to, text, () => view.state.tr.insertText(text, from, to)));
      if (!handled) view.dispatch(view.state.tr.insertText(text, from, to));
    });
  };
  return { ref, type, onSearchNotes, onSearchEntities };
}

describe("note link autocomplete", () => {
  beforeEach(() => {
    // jsdom supplies no text-range geometry or scrolling. These are browser APIs,
    // not measurements used to assert the UI's appearance.
    Object.defineProperty(Range.prototype, "getClientRects", { configurable: true, value: () => [] });
    Object.defineProperty(Range.prototype, "getBoundingClientRect", { configurable: true, value: () => new DOMRect(10, 10, 1, 20) });
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { configurable: true, value: () => {} });
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(10, 10, 300, 30));
    vi.spyOn(Element.prototype, "clientWidth", "get").mockReturnValue(1024);
    vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(768);
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it.each(["name@example.com", "(@Ada", "https://example.com/@Ada", "@ ", "hello@@Ada"])("keeps %s literal", (text) => {
    expect(NOTE_LINK_TRIGGER.test(text)).toBe(false);
  });

  it("groups contacts and mail and inserts an escaped contact reference on selection", async () => {
    const { type, ref, onSearchNotes, onSearchEntities } = await mount();
    await type("@Ada");
    await waitFor(() => expect(onSearchEntities).toHaveBeenCalledWith("Ada"));
    const contacts = await screen.findByRole("group", { name: "Contacts" });
    expect(within(contacts).getByText(contact.label)).toBeInTheDocument();
    expect(within(contacts).getByText(contact.detail)).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Mail" })).toBeInTheDocument();
    expect(onSearchEntities).toHaveBeenCalledWith("Ada");
    expect(onSearchNotes).not.toHaveBeenCalled();
    fireEvent.click(within(contacts).getByRole("option"));
    await waitFor(() => expect(ref.current!.getMarkdown()).toContain(markdownEntityLink(contact.label, contact.reference)));
    expect(ref.current!.getMarkdown()).not.toContain("@Ada");
    expect(ref.current!.getMarkdown()).not.toContain("[[");
  });

  it("selects a conversation with the keyboard and keeps its provider and account identity", async () => {
    const { type, ref } = await mount();
    await type("@review");
    await screen.findByRole("group", { name: "Mail" });
    const box = ref.current!.getEditor()!.view.dom;
    fireEvent.keyDown(box, { key: "ArrowDown" });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(ref.current!.getMarkdown()).toContain(markdownEntityLink(conversation.label, conversation.reference)));
  });

  it("keeps [[ searching notes and inserts a note wiki link", async () => {
    const { type, ref, onSearchNotes, onSearchEntities } = await mount();
    await type("[[Ada");
    const notes = await screen.findByRole("group", { name: "Notes" });
    expect(onSearchNotes).toHaveBeenCalledWith("Ada");
    expect(onSearchEntities).not.toHaveBeenCalled();
    fireEvent.click(within(notes).getByRole("option"));
    await waitFor(() => expect(ref.current!.getMarkdown()).toContain("[[Ada's notes]]"));
  });

  it("dismisses on Escape without inserting or searching ordinary email addresses", async () => {
    const { type, ref, onSearchEntities } = await mount();
    await type("name@example.com ");
    expect(onSearchEntities).not.toHaveBeenCalled();
    await type("@Ada");
    await screen.findByRole("group", { name: "Contacts" });
    fireEvent.keyDown(ref.current!.getEditor()!.view.dom, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("option")).not.toBeInTheDocument());
    expect(ref.current!.getMarkdown()).toContain("name@example.com @Ada");
  });

  it("ignores late search results after the query changes", async () => {
    let resolveFirst!: (rows: NoteLinkChoice[]) => void;
    const first = new Promise<NoteLinkChoice[]>((resolve) => { resolveFirst = resolve; });
    const onSearchEntities = vi.fn(async (query: string) => query === "Ada" ? first : [conversation]);
    const { type } = await mount({ onSearchEntities });
    await type("@Ada");
    await waitFor(() => expect(onSearchEntities).toHaveBeenCalledWith("Ada"));
    await type(" review");
    await screen.findByRole("group", { name: "Mail" });
    await act(async () => resolveFirst([contact]));
    expect(screen.queryByRole("group", { name: "Contacts" })).not.toBeInTheDocument();
  });

  it("shows search errors and recovers when the query changes", async () => {
    const onSearchEntities = vi.fn().mockRejectedValueOnce(new Error("Search unavailable")).mockResolvedValue([contact]);
    const { type } = await mount({ onSearchEntities });
    await type("@Ada");
    expect(await screen.findByText("Search unavailable")).toBeInTheDocument();
    await type(" Work");
    expect(await screen.findByRole("group", { name: "Contacts" })).toBeInTheDocument();
  });

  it("does not autocomplete inside a code block", async () => {
    const { type, ref, onSearchEntities } = await mount({ initial: "```text\nliteral \n```" });
    await type("@Ada");
    expect(ref.current!.getMarkdown()).toContain("literal @Ada");
    expect(onSearchEntities).not.toHaveBeenCalled();
    expect(screen.queryByRole("option")).not.toBeInTheDocument();
  });

  it("leaves code containing an email or @ unchanged on load", async () => {
    const initial = "A literal `name@example.com @Ada` stays text.";
    const { ref, onSearchEntities, onSearchNotes } = await mount({ initial });
    expect(ref.current!.getMarkdown()).toBe(`${initial}\n`);
    expect(onSearchEntities).not.toHaveBeenCalled();
    expect(onSearchNotes).not.toHaveBeenCalled();
  });

  it("keeps @ typed inside inline code literal", async () => {
    const { type, ref, onSearchEntities } = await mount({ initial: "A `code ` tail" });
    act(() => ref.current!.setSelection({ type: "text", anchor: 9, head: 9 }));
    await type("@Ada");
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 200)); });
    expect(ref.current!.getMarkdown()).toBe("A `code @Ada` tail\n");
    expect(onSearchEntities).not.toHaveBeenCalled();
    expect(screen.queryByRole("option")).not.toBeInTheDocument();
  });

  it("replaces just the trigger and preserves text after the cursor", async () => {
    const { type, ref } = await mount({ initial: "Before  after" });
    act(() => ref.current!.setSelection({ type: "text", anchor: 8, head: 8 }));
    await type("@Ada");
    const contacts = await screen.findByRole("group", { name: "Contacts" });
    fireEvent.click(within(contacts).getByRole("option"));
    await waitFor(() => expect(ref.current!.getMarkdown()).toBe(`Before ${markdownEntityLink(contact.label, contact.reference)} after\n`));
  });
});
