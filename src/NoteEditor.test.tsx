import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createRef } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { NoteEditor, linkMenuItems, type NoteEditorHandle } from "./NoteEditor";
import { noteLinkTarget, openNoteLink } from "./noteLinks";
import type { NoteSyntaxMode } from "./noteSyntaxMode";
import type { NoteFiles } from "./noteAttachments";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

function renderEditor(value: string, { mode = "hide", readOnly = false, files }: { mode?: NoteSyntaxMode; readOnly?: boolean; files?: NoteFiles } = {}) {
  const onChange = vi.fn();
  const onLeave = vi.fn();
  const onSelectionChange = vi.fn();
  const ref = createRef<NoteEditorHandle>();
  const view = render(<NoteEditor ref={ref} noteKey="note-1" value={value} label="Note text" mode={mode} readOnly={readOnly} files={files} onChange={onChange} onLeave={onLeave} onSelectionChange={onSelectionChange} />);
  return { ...view, ref, onChange, onLeave, onSelectionChange };
}

async function editorFor(value: string, options?: Parameters<typeof renderEditor>[1]) {
  const rendered = renderEditor(value, options);
  const box = await screen.findByRole("textbox", { name: "Note text" });
  await waitFor(() => expect(rendered.ref.current?.markdown()).toBeDefined());
  return { ...rendered, box };
}

/** Changes the text of the editor's first paragraph the way typing does: in the page, for the editor to read back. */
async function typeOver(box: HTMLElement, text: string) {
  const node = box.querySelector("p")?.firstChild;
  if (!(node instanceof Text)) throw new Error("No paragraph text to type into");
  await act(async () => {
    node.data = text;
    await new Promise((resolve) => setTimeout(resolve, 20));
  });
}

const ID = "0123456789abcdef0123456789abcdef";
const PIXEL = "data:image/png;base64,iVBORw0KGgo=";

function fakeFiles() {
  return {
    save: vi.fn(async () => `attachment:${ID}`),
    image: vi.fn(async (src: string) => src === `attachment:${ID}` ? PIXEL : undefined),
    info: vi.fn(async () => ({ size: 2048 })),
    open: vi.fn(),
  } satisfies NoteFiles;
}

const TASK_ID = "4ec77948-4813-5e64-93f9-c660ae48947f";

/** Markdown that must come back unchanged after loading and saving. */
const ROUND_TRIP: Record<string, string> = {
  headings: "# One\n\n## Two\n\nText under.\n",
  "nested lists": "- a\n  - b\n    - c\n- d\n",
  "ordered list": "1. one\n2. two\n",
  "round task list": "- ( ) todo\n- (x) done\n",
  "persisted round task": "- ( ) Proposal <!-- threestrands-task:4ec77948-4813-5e64-93f9-c660ae48947f -->\n",
  "carried plain item": "- Free text <!-- threestrands-plan:carried 2026-11-16 -->\n",
  "carried link item": "- [Report](https://example.com/x) <!-- threestrands-plan:carried 2026-11-16 -->\n",
  "dropped item": "- Gone <!-- threestrands-plan:dropped -->\n",
  "task id then plan marker": `- ( ) Proposal <!-- threestrands-task:${TASK_ID} --> <!-- threestrands-plan:carried 2026-11-16 -->\n`,
  "plan marker then task id": `- ( ) Proposal <!-- threestrands-plan:deferred 2026-11-20 --> <!-- threestrands-task:${TASK_ID} -->\n`,
  "follow-through marker": "- Follow-through 2026-11-16: [Sync](https://example.com/s) <!-- threestrands-follow-through -->\n  - Send notes\n",
  "plan section": "## Plan\n\n- [A](https://example.com/a) <!-- threestrands-plan:carried 2026-11-16 -->\n  - nested\n- Plain\n- ( ) Mine\n\n### Not today\n\n- B <!-- threestrands-plan:deferred 2026-11-20 -->\n- C <!-- threestrands-plan:dropped -->\n",
  "ordered plan item": "1. One <!-- threestrands-plan:carried 2026-11-16 -->\n2. Two\n",
  "malformed plan marker": "- Free <!-- threestrands-plan:carried tomorrow -->\n",
  "plan marker in fence": "```\n- x <!-- threestrands-plan:dropped -->\n```\n",
  "compact checklists": "- [] Milk\n",
  "compact round tasks": "- () Send mail\n",
  "uppercase checked items": "- [X] Checklist\n- (X) Task\n",
  "task list": "- [ ] todo\n- [x] done\n",
  "code block": "```js\nconst a = 1;\n```\n",
  links: "See [site](https://example.com) and <https://example.org>.\n",
  "wiki link": "Met [[Ada Lovelace]] today.\n",
  "front matter": "---\ntitle: x\n---\n\nBody\n",
  emphasis: "*a* _b_ **c** __d__\n",
  "star bullets": "* x\n* y\n",
};

describe("NoteEditor", () => {
  afterEach(() => { cleanup(); vi.clearAllMocks(); });

  it.each(Object.entries(ROUND_TRIP))("keeps %s unchanged through load and save", async (_name, markdown) => {
    const { ref, onChange } = await editorFor(markdown);
    expect(ref.current!.markdown()).toBe(markdown);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("renders square checklists and round tasks as different list markers", async () => {
    const { box, onChange } = await editorFor("- [ ] Shopping\n- ( ) Proposal\n");
    const items = box.querySelectorAll('[data-list-kind="task"]');
    expect(items).toHaveLength(2);
    expect(items[0]).not.toHaveAttribute("data-list-marker", "+");
    expect(items[1]).toHaveAttribute("data-list-marker", "+");
    expect(await screen.findByRole("checkbox", { name: "Checklist: Shopping" })).not.toBeChecked();
    expect(await screen.findByRole("checkbox", { name: "Task: Proposal" })).not.toBeChecked();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("hides task identity metadata in the editor without losing it on save", async () => {
    const id = "4ec77948-4813-5e64-93f9-c660ae48947f";
    const source = `- ( ) Proposal <!-- threestrands-task:${id} -->\n`;
    const { box, ref } = await editorFor(source);
    expect(box.querySelector("[data-html-comment]")).toBeInTheDocument();
    expect(box.textContent).not.toContain(id);
    expect(ref.current!.markdown()).toBe(source);
  });

  describe.each(["hide", "hybrid", "show"] as const)("inert plan markers in %s mode", (mode) => {
    const carried = "<!-- threestrands-plan:carried 2026-11-16 -->";
    it.each([
      ["a plain item", `- Free text ${carried}\n`],
      ["a link item", `- [Report](https://example.com/x) ${carried}\n`],
      ["a dropped item", "- Gone <!-- threestrands-plan:dropped -->\n"],
      ["a task id then plan marker", `- ( ) Proposal <!-- threestrands-task:${TASK_ID} --> ${carried}\n`],
      ["an ordered item", `1. First ${carried}\n2. Second\n`],
      ["a follow-through section line", "- Follow-through 2026-11-16: [Sync](https://example.com/s) <!-- threestrands-follow-through -->\n"],
    ])("hides the marker on %s and saves it byte for byte", async (_name, source) => {
      const { box, ref, onChange } = await editorFor(source, { mode });
      expect(box.textContent).not.toContain("threestrands-");
      expect(box.textContent).not.toContain("<!--");
      // The rest of the line is parsed, not shown as one unparsed run.
      if (source.includes("](")) expect(box.querySelector("a.md-link")).not.toBeNull();
      expect(ref.current!.markdown()).toBe(source);
      expect(onChange).not.toHaveBeenCalled();
    });
    it("keeps the marker on its item when the item's text is edited", async () => {
      const { box, ref } = await editorFor(`- Free text ${carried}\n- Other\n`, { mode });
      const node = box.querySelector(".list-content p")?.firstChild;
      if (!(node instanceof Text)) throw new Error("No item text");
      await act(async () => { node.data = "Edited text"; await new Promise((resolve) => setTimeout(resolve, 20)); });
      expect(ref.current!.markdown()).toBe(`- Edited text ${carried}\n- Other\n`);
    });
    it("still finds a task whose id sits in a hidden marker run", async () => {
      const { box, ref } = await editorFor(`- First\n- ( ) Proposal <!-- threestrands-task:${TASK_ID} --> ${carried}\n`, { mode });
      // jsdom has no layout; scrolling to the new selection measures it.
      Object.defineProperty(Range.prototype, "getClientRects", { configurable: true, value: () => Object.assign([], { item: () => null }) });
      Object.defineProperty(Range.prototype, "getBoundingClientRect", { configurable: true, value: () => new DOMRect() });
      onTestFinished(() => { delete (Range.prototype as { getClientRects?: unknown }).getClientRects; delete (Range.prototype as { getBoundingClientRect?: unknown }).getBoundingClientRect; });
      act(() => ref.current!.revealTask(TASK_ID));
      const items = box.querySelectorAll(".list-content");
      await waitFor(() => expect(items[1].contains(document.getSelection()?.anchorNode ?? null)).toBe(true));
    });
    it("leaves a malformed marker and one inside a fence as authored content", async () => {
      const source = "- Free <!-- threestrands-plan:carried tomorrow -->\n\n```\n- x <!-- threestrands-plan:dropped -->\n```\n";
      const { box, ref } = await editorFor(source, { mode });
      expect(box.textContent).toContain("threestrands-plan:dropped");
      expect(ref.current!.markdown()).toBe(source);
    });
  });

  it("prevents internal reference links from navigating on a middle click", async () => {
    const { noteReferenceHref } = await import("./noteEntities");
    const href = noteReferenceHref({ version: 1, kind: "task", id: "task-id" });
    const { container } = await editorFor(`[Task](${href})\n`);
    const anchor = container.querySelector("a[href]")!;
    expect(anchor).not.toBeNull();
    const event = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
    anchor.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(openUrl).not.toHaveBeenCalled();
  });

  it("writes a table's delimiter row in its own form", async () => {
    const { ref } = await editorFor("| a | b |\n| - | - |\n| 1 | 2 |\n");
    expect(ref.current!.markdown()).toBe("| a | b |\n| --- | --- |\n| 1 | 2 |\n");
  });

  it("reports text only after the user changes it", async () => {
    const { box, onChange } = await editorFor("Hello world\n");
    expect(onChange).not.toHaveBeenCalled();
    await typeOver(box, "Hello there world");
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenLastCalledWith("Hello there world\n");
  });

  describe("Mail formatting shortcuts", () => {
    beforeEach(() => {
      // jsdom has no text-range geometry; editor commands scroll the selection.
      Object.defineProperty(Range.prototype, "getClientRects", { configurable: true, value: () => [] });
      Object.defineProperty(Range.prototype, "getBoundingClientRect", { configurable: true, value: () => new DOMRect() });
    });
    it.each([
      ["bold", "b", false, "**Hello**\n"],
      ["italic", "i", false, "*Hello*\n"],
      ["strikethrough", "x", true, "~~Hello~~\n"],
      ["fixed-width", "m", true, "`Hello`\n"],
      ["numbered list", "7", true, "1. Hello\n"],
      ["bulleted list", "8", true, "- Hello\n"],
      ["quote", "9", true, "> Hello\n"],
      ["code block", "0", true, "```\nHello\n```\n"],
    ])("applies %s to selected text and supports undo", async (_name, key, shiftKey, expected) => {
      const { box, ref, onChange } = await editorFor("Hello\n");
      act(() => ref.current!.focus());
      fireEvent.keyDown(box, { key: "a", ctrlKey: true });
      fireEvent.keyDown(box, { key, shiftKey, ctrlKey: true });
      expect(ref.current!.markdown()).toBe(expected);
      expect(onChange).toHaveBeenLastCalledWith(expected);
      fireEvent.keyDown(box, { key: "z", ctrlKey: true });
      expect(ref.current!.markdown()).toBe("Hello\n");
    });

    it.each(["# Heading\n", "```js\nCode\n```\n"])("returns %s to normal text with Mod-0", async (source) => {
      const { box, ref } = await editorFor(source);
      act(() => ref.current!.focus());
      fireEvent.keyDown(box, { key: "a", ctrlKey: true });
      fireEvent.keyDown(box, { key: "0", metaKey: true });
      expect(ref.current!.markdown()).toBe(source.includes("Heading") ? "Heading\n" : "Code\n");
      expect(box.querySelector("h1, pre")).toBeNull();
      fireEvent.keyDown(box, { key: "z", ctrlKey: true });
      expect(ref.current!.markdown()).toBe(source);
    });

    it.each([
      ["**Bold** and *italic* and ~~strike~~ and `code` and [link](https://example.com)\n", "Bold and italic and strike and code and link\n"],
      ["***Nested*** and ==highlight==\n", "Nested and highlight\n"],
      ["**First**\n\n*Second*\n", "First\n\nSecond\n"],
      ["- [ ] **Checklist**\n- ( ) *Task*\n", "- [ ] Checklist\n- ( ) Task\n"],
    ])("clears inline formatting from %s with one undoable shortcut", async (source, plain) => {
      const { box, ref } = await editorFor(source);
      act(() => ref.current!.focus());
      fireEvent.keyDown(box, { key: "a", ctrlKey: true });
      fireEvent.keyDown(box, { key: "a", ctrlKey: true });
      fireEvent.keyDown(box, { key: "\\", metaKey: true });
      expect(ref.current!.markdown()).toBe(plain);
      expect(box.querySelector("strong, em, del, code, a.md-link, mark")).toBeNull();
      fireEvent.keyDown(box, { key: "z", ctrlKey: true });
      expect(ref.current!.markdown()).toBe(source);
    });

    it("clears only the selected part of bold text and keeps the selection", async () => {
      const { box, ref } = await editorFor("**Hello world**\n");
      act(() => ref.current!.focus());
      const node = box.querySelector("strong")!.childNodes[1];
      const range = document.createRange(); range.setStart(node, 0); range.setEnd(node, 5);
      window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range);
      // Let ProseMirror read the browser selection before applying the command.
      fireEvent(document, new Event("selectionchange"));
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 30)); });
      fireEvent.keyDown(box, { key: "\\", ctrlKey: true });
      expect(ref.current!.markdown()).toBe("Hello **world**\n");
      expect(window.getSelection()!.toString()).toBe("Hello");
    });

    it("reports text selection only in the editable note and clears it on blur, read-only mode, and unmount", async () => {
      const { box, ref, onSelectionChange, rerender, unmount } = await editorFor("Hello\n");
      act(() => ref.current!.focus());
      const range = document.createRange(); range.selectNodeContents(box);
      window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range);
      act(() => document.dispatchEvent(new Event("selectionchange")));
      expect(onSelectionChange).toHaveBeenLastCalledWith(true);
      const other = document.createElement("input"); document.body.append(other);
      act(() => other.focus());
      expect(onSelectionChange).toHaveBeenLastCalledWith(false);
      act(() => ref.current!.focus());
      window.getSelection()!.collapseToEnd();
      act(() => document.dispatchEvent(new Event("selectionchange")));
      expect(onSelectionChange).toHaveBeenLastCalledWith(false);
      rerender(<NoteEditor ref={ref} noteKey="note-1" value="Hello\n" label="Note text" mode="hide" readOnly onChange={vi.fn()} onLeave={vi.fn()} onSelectionChange={onSelectionChange} />);
      expect(onSelectionChange).toHaveBeenLastCalledWith(false);
      unmount();
      expect(onSelectionChange).toHaveBeenLastCalledWith(false);
    });

    it.each(["m", "9", "0"])("does not apply %s while composing or reading a note", async (key) => {
      const { box, ref, onChange } = await editorFor("Hello\n");
      fireEvent.keyDown(box, { key, ctrlKey: true, shiftKey: true, isComposing: true });
      expect(ref.current!.markdown()).toBe("Hello\n");
      expect(onChange).not.toHaveBeenCalled();
      cleanup();
      const readOnly = await editorFor("Hello\n", { readOnly: true });
      fireEvent.keyDown(readOnly.box, { key, ctrlKey: true, shiftKey: true });
      expect(readOnly.ref.current!.markdown()).toBe("Hello\n");
      expect(readOnly.onChange).not.toHaveBeenCalled();
    });

    it.each([
      ["m", "KeyM", "`Hello`\n"],
      ["(", "Digit9", "> Hello\n"],
      [")", "Digit0", "```\nHello\n```\n"],
    ])("supports the Mac %s binding and toggles it off again", async (key, code, expected) => {
      const { box, ref } = await editorFor("Hello\n");
      act(() => ref.current!.focus());
      fireEvent.keyDown(box, { key: "a", ctrlKey: true });
      fireEvent.keyDown(box, { key, code, shiftKey: true, metaKey: true });
      expect(ref.current!.markdown()).toBe(expected);
      fireEvent.keyDown(box, { key, code, shiftKey: true, metaKey: true });
      expect(ref.current!.markdown()).toBe("Hello\n");
    });

    it("inserts a hyperlink around selected text with Mod+k", async () => {
      const { box, ref, onChange } = await editorFor("Hello\n");
      act(() => ref.current!.focus());
      fireEvent.keyDown(box, { key: "a", ctrlKey: true });
      fireEvent.keyDown(box, { key: "k", ctrlKey: true });
      const url = await screen.findByRole("textbox", { name: "Link" });
      expect(screen.getByRole("textbox", { name: "Text" })).toHaveValue("Hello");
      fireEvent.change(url, { target: { value: "https://example.com" } });
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      expect(ref.current!.markdown()).toBe("[Hello](https://example.com)\n");
      expect(onChange).toHaveBeenLastCalledWith("[Hello](https://example.com)\n");
    });

    it.each([
      ["Tab", {}, "Tab", { shiftKey: true }],
      ["]", { ctrlKey: true }, "[", { ctrlKey: true }],
    ])("indents and outdents list items with %s", async (indent, indentModifiers, outdent, outdentModifiers) => {
      const { box, ref } = await editorFor("- One\n- Two\n");
      act(() => ref.current!.focus());
      fireEvent.keyDown(box, { key: "ArrowDown", ctrlKey: true });
      fireEvent.keyDown(box, { key: indent, ...indentModifiers });
      expect(ref.current!.markdown()).toBe("- One\n  - Two\n");
      fireEvent.keyDown(box, { key: outdent, ...outdentModifiers });
      expect(ref.current!.markdown()).toBe("- One\n- Two\n");
    });
  });

  describe("pasting", () => {
    afterEach(() => vi.unstubAllGlobals());
    beforeEach(() => {
      // jsdom has no text-range geometry; a paste scrolls the selection into view.
      Object.defineProperty(Range.prototype, "getClientRects", { configurable: true, value: () => [] });
      Object.defineProperty(Range.prototype, "getBoundingClientRect", { configurable: true, value: () => new DOMRect() });
    });
    it("pastes mail HTML without writing spacing as character references", async () => {
      const { box, ref } = await editorFor("");
      act(() => ref.current!.focus());
      const html = '<p><b>Complimentary Parking: </b><a href="https://maps.example/x"><i>1250 Bedford Ave.</i></a></p><p><b> </b></p><p><i>Parking Instructions</i></p>';
      fireEvent.paste(box, { clipboardData: { files: [], types: ["text/html", "text/plain"], getData: (type: string) => type === "text/html" ? html : "Complimentary Parking: 1250 Bedford Ave.\n\nParking Instructions" } });
      await waitFor(() => expect(ref.current!.markdown()).toContain("Parking Instructions"));
      const markdown = ref.current!.markdown()!;
      expect(markdown).not.toMatch(/&#x?[0-9a-f]+;/i);
      expect(markdown).toContain("**Complimentary Parking:** [*1250 Bedford Ave.*](https://maps.example/x)");
    });

    it.each(["Plain <b>text</b>\nSecond line", "Plain clipboard text"])("pastes %s as plain text with Cmd-Shift-V and supports undo", async (text) => {
      const readText = vi.fn().mockResolvedValue(text);
      vi.stubGlobal("navigator", new Proxy(navigator, { get: (target, key) => key === "clipboard" ? { readText } : Reflect.get(target, key, target) }));
      const { box, ref, onChange } = await editorFor("Replace me\n");
      act(() => ref.current!.focus());
      fireEvent.keyDown(box, { key: "a", ctrlKey: true });
      fireEvent.keyDown(box, { key: "V", metaKey: true, shiftKey: true });
      await waitFor(() => expect(onChange).toHaveBeenCalled());
      expect(box).toHaveTextContent(text.replace("\n", ""));
      expect(box.querySelector("strong, b, a, img")).toBeNull();
      fireEvent.keyDown(box, { key: "z", ctrlKey: true });
      expect(ref.current!.markdown()).toBe("Replace me\n");
    });
  });

  it("shows text from outside without reporting it as an edit", async () => {
    const { rerender, ref, onChange } = await editorFor("first\n");
    rerender(<NoteEditor ref={ref} noteKey="note-1" value={"from another device\n"} label="Note text" mode="hide" onChange={onChange} onLeave={vi.fn()} />);
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Note text" })).toHaveTextContent("from another device"));
    expect(ref.current!.markdown()).toBe("from another device\n");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("starts undo history at text from outside, so undo can't bring back what it replaced", async () => {
    const { rerender, ref, onChange } = await editorFor("first\n");
    rerender(<NoteEditor ref={ref} noteKey="note-1" value={"second\n"} label="Note text" mode="hide" onChange={onChange} onLeave={vi.fn()} />);
    const box = await screen.findByRole("textbox", { name: "Note text" });
    await waitFor(() => expect(box).toHaveTextContent("second"));
    for (const key of [{ ctrlKey: true }, { metaKey: true }]) {
      screen.getByRole("textbox", { name: "Note text" }).dispatchEvent(new KeyboardEvent("keydown", { key: "z", bubbles: true, cancelable: true, ...key }));
    }
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
    expect(ref.current!.markdown()).toBe("second\n");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("shows a link's shown text and keeps its Markdown", async () => {
    const { box, ref } = await editorFor("Met [[Ada Lovelace|Ada]] and [[Charles Babbage]].\n");
    const chips = [...box.querySelectorAll(".md-wikilink-view-preview")].map((chip) => chip.textContent);
    expect(chips).toEqual(["Ada", "Charles Babbage"]);
    expect(ref.current!.markdown()).toBe("Met [[Ada Lovelace|Ada]] and [[Charles Babbage]].\n");
  });

  it("offers matching notes in the link picker, then the typed title as a new note", () => {
    expect(linkMenuItems("road  trip", [{ title: "Road trip planning" }])).toEqual([
      { target: "Road trip planning", detail: undefined },
      { target: "road trip", detail: "New note" },
    ]);
    expect(linkMenuItems("ROAD TRIP", [{ title: "Road trip" }])).toEqual([{ target: "Road trip", detail: undefined }]);
    expect(linkMenuItems("", [{ title: "Recent" }])).toEqual([{ target: "Recent", detail: undefined }]);
    // A title that can't be a note's isn't offered as one.
    expect(linkMenuItems("a|b", [])).toEqual([]);
  });

  it("is read-only when asked and shows the chosen syntax mode", async () => {
    const { box } = await editorFor("text\n", { readOnly: true, mode: "show" });
    expect(box).toHaveAttribute("contenteditable", "false");
    expect(box).toHaveAttribute("data-mark-mode", "show");
    cleanup();
    expect((await editorFor("text\n", { mode: "hybrid" })).box).toHaveAttribute("data-mark-mode", "focus");
  });

  it("returns to the list on Escape and keeps the key from reaching other shortcuts", async () => {
    const { box, onLeave } = await editorFor("text\n");
    const escape = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    box.dispatchEvent(escape);
    expect(onLeave).toHaveBeenCalledTimes(1);
    expect(escape.defaultPrevented).toBe(true);
  });

  describe("attached files", () => {
    it("shows a file as a pill with its size and keeps its Markdown", async () => {
      const files = fakeFiles();
      const markdown = `See [Plan.pdf](attachment:${ID}) and [site](attachment:not-an-id).\n`;
      const { ref, container, onChange } = await editorFor(markdown, { files });
      const pill = await waitFor(() => {
        const found = container.querySelector(".md-file-view");
        expect(found).not.toBeNull();
        return found!;
      });
      expect(pill).toHaveTextContent("Plan.pdf");
      await waitFor(() => expect(pill).toHaveTextContent("2 KB"));
      expect(files.info).toHaveBeenCalledWith(`attachment:${ID}`);
      // Only a real attachment id is claimed as a file; the rest stays a link.
      expect([...new Set([...container.querySelectorAll("a")].map((link) => link.getAttribute("href")))]).toEqual(["attachment:not-an-id"]);
      expect(ref.current!.markdown()).toBe(markdown);
      expect(onChange).not.toHaveBeenCalled();
    });
  });

  describe("untrusted note text", () => {
    it("shows raw HTML as text and never creates live elements", async () => {
      const { container, box } = await editorFor("<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n\n<iframe src=\"https://example.com\"></iframe>\n");
      expect(box).toHaveTextContent("<script>alert(1)</script>");
      expect(box).toHaveTextContent("<img src=x onerror=alert(1)>");
      expect(container.querySelectorAll("script, img, iframe")).toHaveLength(0);
    });

    it("loads no remote images or embeds", async () => {
      const fetchSpy = vi.spyOn(globalThis, "fetch");
      const { container } = await editorFor("![photo](https://example.com/a.png)\n\n![](https://x.com/someone/status/123)\n\n![](https://www.youtube.com/watch?v=abc)\n");
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
      expect(container.querySelectorAll("img, iframe")).toHaveLength(0);
      expect(fetchSpy).not.toHaveBeenCalled();
      fetchSpy.mockRestore();
    });

    it("shows only attached images, never remote ones, even with files to show", async () => {
      const fetchSpy = vi.spyOn(globalThis, "fetch");
      const files = fakeFiles();
      const { container } = await editorFor(`![photo](https://example.com/a.png)\n\n![](attachment:${ID})\n`, { files });
      await waitFor(() => expect(container.querySelectorAll("img")).toHaveLength(1));
      expect(container.querySelector("img")!.getAttribute("src")).toBe(PIXEL);
      expect(files.image).toHaveBeenCalledWith(`attachment:${ID}`);
      expect(fetchSpy).not.toHaveBeenCalled();
      fetchSpy.mockRestore();
    });

    it("never lets a link navigate the app window, even read-only", async () => {
      const { container } = await editorFor("[safe](https://example.com) [bad](javascript:alert(1))\n", { readOnly: true });
      const links = [...container.querySelectorAll("a")];
      expect(links.map((link) => link.getAttribute("href"))).toEqual(["https://example.com", "javascript:alert(1)"]);
      for (const link of links) {
        const click = new MouseEvent("click", { bubbles: true, cancelable: true });
        link.dispatchEvent(click);
        expect(click.defaultPrevented).toBe(true);
      }
      expect(openUrl).not.toHaveBeenCalled();
    });
  });
});

describe("note links", () => {
  it("allows web and email addresses and nothing else", () => {
    expect(noteLinkTarget("https://example.com/a")).toBe("https://example.com/a");
    expect(noteLinkTarget(" http://example.com ")).toBe("http://example.com/");
    expect(noteLinkTarget("www.example.com")).toBe("https://www.example.com/");
    expect(noteLinkTarget("mailto:a@example.com")).toBe("mailto:a@example.com");
    for (const href of ["javascript:alert(1)", "JAVASCRIPT:alert(1)", "file:///etc/passwd", "data:text/html,x", "threestrands://x", "relative/path", ""]) {
      expect(noteLinkTarget(href)).toBeNull();
    }
  });

  it("opens web links in the browser and ignores the rest", () => {
    openNoteLink("javascript:alert(1)");
    openNoteLink("file:///etc/passwd");
    expect(openUrl).not.toHaveBeenCalled();
    openNoteLink("https://example.com/page");
    expect(openUrl).toHaveBeenCalledWith("https://example.com/page");
  });
});
