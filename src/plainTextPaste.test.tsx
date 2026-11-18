import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { pasteWithoutFormatting } from "./plainTextPaste";

afterEach(() => vi.unstubAllGlobals());

function setup(readText = vi.fn().mockResolvedValue("Plain\r\ntext")) {
  vi.stubGlobal("navigator", new Proxy(navigator, { get: (target, key) => key === "clipboard" ? { readText } : Reflect.get(target, key, target) }));
  const insert = vi.fn();
  const onError = vi.fn();
  render(<div onKeyDown={(event) => pasteWithoutFormatting(event, insert, onError)}>
    <div role="textbox" contentEditable suppressContentEditableWarning>Selected text</div><input aria-label="Other field" />
  </div>);
  const editor = screen.getByRole("textbox", { name: "" });
  editor.focus();
  const range = document.createRange();
  range.selectNodeContents(editor);
  window.getSelection()!.removeAllRanges();
  window.getSelection()!.addRange(range);
  return { editor, insert, onError, readText };
}

describe("paste without formatting", () => {
  it.each([{ metaKey: true }, { ctrlKey: true }])("reads text once and cancels native paste with %j", async (modifiers) => {
    const { editor, readText, insert } = setup();
    expect(fireEvent.keyDown(editor, { key: "V", shiftKey: true, ...modifiers })).toBe(false);
    expect(readText).toHaveBeenCalledOnce();
    await act(async () => {});
    expect(insert).toHaveBeenCalledExactlyOnceWith("Plain\ntext", editor);
  });

  it.each([
    { key: "v", metaKey: true },
    { key: "v", shiftKey: true },
    { key: "v", metaKey: true, shiftKey: true, altKey: true },
    { key: "b", metaKey: true, shiftKey: true },
  ])("leaves other shortcuts and ordinary paste unchanged (%j)", async (keys) => {
    const { editor, readText, insert } = setup();
    expect(fireEvent.keyDown(editor, keys)).toBe(true);
    expect(fireEvent.paste(editor)).toBe(true);
    await act(async () => {});
    expect(readText).not.toHaveBeenCalled();
    expect(insert).not.toHaveBeenCalled();
  });

  it.each([{ repeat: true }, { isComposing: true }])("ignores repeated keys and composition (%j)", (keys) => {
    const { editor, readText } = setup();
    fireEvent.keyDown(editor, { key: "v", metaKey: true, shiftKey: true, ...keys });
    expect(readText).not.toHaveBeenCalled();
  });

  it("leaves ordinary text fields and read-only editors to native handling", () => {
    const { editor, readText } = setup();
    const keys = { key: "v", metaKey: true, shiftKey: true };
    expect(fireEvent.keyDown(screen.getByRole("textbox", { name: "Other field" }), keys)).toBe(true);
    editor.setAttribute("contenteditable", "false");
    expect(fireEvent.keyDown(editor, keys)).toBe(true);
    expect(readText).not.toHaveBeenCalled();
  });

  it.each(["focus", "selection", "content", "removed", "read-only"])("does not insert after the %s changes while reading", async (change) => {
    let resolve!: (text: string) => void;
    const { editor, insert } = setup(vi.fn(() => new Promise<string>((done) => { resolve = done; })));
    fireEvent.keyDown(editor, { key: "v", metaKey: true, shiftKey: true });
    if (change === "focus") screen.getByRole("textbox", { name: "Other field" }).focus();
    if (change === "selection") window.getSelection()!.collapseToEnd();
    if (change === "content") editor.append("more");
    if (change === "removed") editor.remove();
    if (change === "read-only") editor.setAttribute("contenteditable", "false");
    await act(async () => { resolve("Late clipboard"); });
    expect(insert).not.toHaveBeenCalled();
  });

  it("keeps the selection on an empty or non-text clipboard", async () => {
    const { editor, insert } = setup(vi.fn().mockResolvedValue(""));
    fireEvent.keyDown(editor, { key: "v", metaKey: true, shiftKey: true });
    await act(async () => {});
    expect(insert).not.toHaveBeenCalled();
    expect(window.getSelection()!.toString()).toBe("Selected text");
  });

  it("reports a rejected clipboard read without changing content", async () => {
    const { editor, insert, onError } = setup(vi.fn().mockRejectedValue(new Error("Clipboard unavailable")));
    fireEvent.keyDown(editor, { key: "v", metaKey: true, shiftKey: true });
    await act(async () => {});
    expect(onError).toHaveBeenCalledWith(expect.stringContaining("Clipboard unavailable"));
    expect(insert).not.toHaveBeenCalled();
    expect(editor).toHaveTextContent("Selected text");
  });
});
