import { afterEach, describe, expect, it, vi } from "vitest";
import {
  applyFormattingShortcut,
  formattingShortcutFor,
  formattingShortcuts,
  plainTextToHtml,
  sanitizeComposeHtml,
  serializeComposeHtml,
} from "./richText";

const event = (key: string, options: KeyboardEventInit = {}) =>
  new KeyboardEvent("keydown", { key, ...options });

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("Superhuman formatting shortcuts", () => {
  it("registers every formatting combo from the Superhuman shortcut sheet", () => {
    expect(formattingShortcuts.map(({ title, key }) => [title, key])).toEqual([
      ["Bold", "Mod+b"],
      ["Italics", "Mod+i"],
      ["Underline", "Mod+u"],
      ["Hyperlink", "Mod+k"],
      ["Color", "Mod+Shift+c"],
      ["Strikethrough", "Mod+Shift+x"],
      ["Numbered list", "Mod+Shift+7"],
      ["Bulleted list", "Mod+Shift+8"],
      ["Quote", "Mod+Shift+9"],
      ["Indent list", "Tab"],
      ["Outdent list", "Shift+Tab"],
      ["Increase indent", "Mod+]"],
      ["Decrease indent", "Mod+["],
    ]);
  });

  it("matches shifted number shortcuts by physical digit key", () => {
    expect(formattingShortcutFor(event("&", { metaKey: true, shiftKey: true, code: "Digit7" }))?.title).toBe("Numbered list");
    expect(formattingShortcutFor(event("*", { ctrlKey: true, shiftKey: true, code: "Digit8" }))?.title).toBe("Bulleted list");
    expect(formattingShortcutFor(event("(", { metaKey: true, shiftKey: true, code: "Digit9" }))?.title).toBe("Quote");
  });

  it("executes formatting and normalizes hyperlink destinations", () => {
    const editor = document.createElement("div");
    document.body.append(editor);
    const execute = vi.fn(() => true);
    Object.defineProperty(document, "execCommand", { configurable: true, value: execute });

    const bold = formattingShortcuts.find(({ id }) => id === "format.bold")!;
    const link = formattingShortcuts.find(({ id }) => id === "format.link")!;
    expect(applyFormattingShortcut(editor, bold)).toBe(true);
    expect(applyFormattingShortcut(editor, link, () => "example.com")).toBe(true);

    expect(execute).toHaveBeenNthCalledWith(1, "bold", false, "");
    expect(execute).toHaveBeenNthCalledWith(2, "createLink", false, "https://example.com");
  });

  it("uses Tab for list indentation only when the caret is in a list item", () => {
    const editor = document.createElement("div");
    editor.innerHTML = "<ul><li>Item</li></ul><p>Outside</p>";
    document.body.append(editor);
    const execute = vi.fn(() => true);
    Object.defineProperty(document, "execCommand", { configurable: true, value: execute });
    const indent = formattingShortcuts.find(({ id }) => id === "format.indentList")!;

    expect(applyFormattingShortcut(editor, indent)).toBe(false);
    const range = document.createRange();
    range.selectNodeContents(editor.querySelector("li")!);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
    expect(applyFormattingShortcut(editor, indent)).toBe(true);
    expect(execute).toHaveBeenCalledWith("indent", false, "");
  });
});

it("converts plain drafts and sanitizes rich compose HTML", () => {
  expect(plainTextToHtml("Hello\nworld")).toBe("Hello<br>world");
  expect(sanitizeComposeHtml('<b>Hi</b><script>alert(1)</script><a href="javascript:alert(1)">bad</a>'))
    .toBe("<b>Hi</b><a>bad</a>");
});

it("keeps safe pasted images and strips compose-only image controls", () => {
  const editor = document.createElement("div");
  editor.innerHTML = '<span data-compose-image="true"><img src="data:image/png;base64,aGVsbG8=" alt="Screenshot" width="320"><button>remove</button><span data-compose-image-resize></span></span>';

  expect(serializeComposeHtml(editor)).toBe('<img src="data:image/png;base64,aGVsbG8=" alt="Screenshot" width="320">');
  expect(sanitizeComposeHtml('<img src="javascript:alert(1)"><img src="data:text/html;base64,aGk=">')).toBe("");
});
