import { afterEach, describe, expect, it, vi } from "vitest";
import {
  applyAsteriskListShortcut,
  applyFormattingShortcut,
  formattingShortcutFor,
  formattingShortcuts,
  insertHtmlAtRange,
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
    // Fixed-Width is an addition beyond the Superhuman sheet: Superhuman has
    // no fixed-width face at all, so the key is ours to choose.
    expect(formattingShortcuts.map(({ title, key }) => [title, key])).toEqual([
      ["Bold", "Mod+b"],
      ["Italics", "Mod+i"],
      ["Underline", "Mod+u"],
      ["Hyperlink", "Mod+k"],
      ["Color", "Mod+Shift+c"],
      ["Strikethrough", "Mod+Shift+x"],
      ["Fixed-Width", "Mod+Shift+m"],
      ["Numbered List", "Mod+Shift+7"],
      ["Bulleted List", "Mod+Shift+8"],
      ["Quote", "Mod+Shift+9"],
      ["Indent List", "Tab"],
      ["Outdent List", "Shift+Tab"],
      ["Increase Indent", "Mod+]"],
      ["Decrease Indent", "Mod+["],
    ]);
  });

  it("matches shifted number shortcuts by physical digit key", () => {
    expect(formattingShortcutFor(event("&", { metaKey: true, shiftKey: true, code: "Digit7" }))?.title).toBe("Numbered List");
    expect(formattingShortcutFor(event("*", { ctrlKey: true, shiftKey: true, code: "Digit8" }))?.title).toBe("Bulleted List");
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

describe("asterisk list shortcut", () => {
  it("turns a standalone asterisk into an empty bulleted list", () => {
    const editor = document.createElement("div");
    editor.textContent = "*";
    document.body.append(editor);
    const text = editor.firstChild!;
    const range = document.createRange();
    range.setStart(text, 1);
    range.collapse(true);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);

    expect(applyAsteriskListShortcut(editor)).toBe(true);
    expect(editor.innerHTML).toBe("<ul><li><br></li></ul>");
    expect(selection.anchorNode).toBe(editor.querySelector("li"));
  });

  it.each(["<p>Hello *</p>", "<ul><li>*</li></ul>", "<p>* more</p>"]) (
    "leaves %s unchanged",
    (html) => {
      const editor = document.createElement("div");
      editor.innerHTML = html;
      document.body.append(editor);
      const text = editor.querySelector("p, li")?.firstChild ?? editor.firstChild!;
      const range = document.createRange();
      range.selectNodeContents(text);
      range.collapse(false);
      const selection = window.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);

      expect(applyAsteriskListShortcut(editor)).toBe(false);
      expect(editor.innerHTML).toBe(html);
    },
  );
});

it("converts plain drafts and sanitizes rich compose HTML", () => {
  expect(plainTextToHtml("Hello\nworld")).toBe("Hello<br>world");
  expect(sanitizeComposeHtml('<b>Hi</b><script>alert(1)</script><a href="javascript:alert(1)">bad</a>'))
    .toBe("<b>Hi</b><a>bad</a>");
});

describe("fixed-width selection", () => {
  const fixedWidth = formattingShortcuts.find(({ id }) => id === "format.fixedWidth")!;

  it("matches ⌘⇧M regardless of the shifted key's case", () => {
    expect(formattingShortcutFor(event("M", { metaKey: true, shiftKey: true }))?.title).toBe("Fixed-Width");
    expect(formattingShortcutFor(event("m", { metaKey: true, shiftKey: true }))?.title).toBe("Fixed-Width");
    expect(formattingShortcutFor(event("m", { metaKey: true }))?.title).not.toBe("Fixed-Width");
  });

  it("applies the monospace face via fontName", () => {
    const editor = document.createElement("div");
    document.body.append(editor);
    const execute = vi.fn(() => true);
    Object.defineProperty(document, "execCommand", { configurable: true, value: execute });

    expect(applyFormattingShortcut(editor, fixedWidth)).toBe(true);
    expect(execute).toHaveBeenCalledWith("fontName", false, "monospace");
  });

  it("strips an inline fixed-width span when the selection is already fixed-width", () => {
    const editor = document.createElement("div");
    editor.innerHTML = '<p>plain <span style="font-family: monospace">code</span></p>';
    document.body.append(editor);
    const execute = vi.fn(() => true);
    Object.defineProperty(document, "execCommand", { configurable: true, value: execute });
    const text = editor.querySelector("span")!.firstChild!;
    const range = document.createRange();
    range.setStart(text, 1);
    range.collapse(true);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);

    expect(applyFormattingShortcut(editor, fixedWidth)).toBe(true);
    expect(execute).not.toHaveBeenCalled();
    expect(editor.innerHTML).toBe("<p>plain code</p>");
  });

  it("toggles off a legacy <font face> marking while keeping its color", () => {
    const editor = document.createElement("div");
    editor.innerHTML = '<font face="monospace" color="#2563eb">code</font>';
    document.body.append(editor);
    const execute = vi.fn(() => true);
    Object.defineProperty(document, "execCommand", { configurable: true, value: execute });
    const text = editor.querySelector("font")!.firstChild!;
    const range = document.createRange();
    range.setStart(text, 1);
    range.collapse(true);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);

    expect(applyFormattingShortcut(editor, fixedWidth)).toBe(true);
    expect(execute).not.toHaveBeenCalled();
    expect(editor.innerHTML).toBe('<font color="#2563eb">code</font>');
  });
});

it("normalizes fixed-width compose HTML through the shared font-family policy", () => {
  // WebKit's fontName output is normalized to inline style, which the
  // message reader already renders, so the marking survives sending.
  const face = document.createElement("div");
  face.innerHTML = sanitizeComposeHtml('<font face="monospace">x</font>');
  expect(face.firstElementChild?.tagName).toBe("SPAN");
  expect((face.firstElementChild as HTMLElement).style.fontFamily).toBe("monospace");
  expect(face.querySelector("font")).toBeNull();

  // A valid family alongside the still-supported margin-left.
  const styled = document.createElement("div");
  styled.innerHTML = sanitizeComposeHtml('<span style="margin-left: 24px; font-family: monospace">x</span>');
  expect((styled.firstElementChild as HTMLElement).style.fontFamily).toBe("monospace");
  expect((styled.firstElementChild as HTMLElement).style.marginLeft).toBe("24px");
});

it("strips hostile font families from compose HTML without losing content", () => {
  const face = document.createElement("div");
  face.innerHTML = sanitizeComposeHtml('<font face="monospace; background:url(https://tracker.invalid/x)">x</font>');
  expect(face.firstElementChild?.tagName).toBe("SPAN");
  expect(face.firstElementChild?.getAttribute("style")).toBeNull();
  expect(face.textContent).toBe("x");

  const styled = document.createElement("div");
  styled.innerHTML = sanitizeComposeHtml('<span style="font-family: expression(alert(1)); font-family: monospace">x</span>');
  const style = styled.firstElementChild?.getAttribute("style") ?? "";
  expect(style).not.toContain("expression");
  expect(style).toContain("font-family");
  expect(styled.textContent).toBe("x");
});

it("keeps safe pasted images and strips compose-only image controls", () => {
  const editor = document.createElement("div");
  editor.innerHTML = '<span data-compose-image="true"><img src="data:image/png;base64,aGVsbG8=" alt="Screenshot" width="320"><button>remove</button><span data-compose-image-resize></span></span>';

  expect(serializeComposeHtml(editor)).toBe('<img src="data:image/png;base64,aGVsbG8=" alt="Screenshot" width="320">');
  expect(sanitizeComposeHtml('<img src="javascript:alert(1)"><img src="data:text/html;base64,aGk=">')).toBe("");
});

describe("insertHtmlAtRange", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("inserts HTML at a collapsed range and leaves the caret just after it", () => {
    const editor = document.createElement("div");
    editor.textContent = "Hello world";
    document.body.append(editor);
    const textNode = editor.firstChild!;
    const range = document.createRange();
    range.setStart(textNode, 5); // "Hello| world"
    range.collapse(true);

    insertHtmlAtRange(editor, range, "<b>!</b>");

    expect(editor.innerHTML).toBe("Hello<b>!</b> world");
    const selection = window.getSelection()!;
    expect(selection.isCollapsed).toBe(true);
    expect(editor.contains(selection.anchorNode)).toBe(true);
  });

  it("replaces a non-collapsed selection with the inserted HTML", () => {
    const editor = document.createElement("div");
    editor.textContent = "Hello world";
    document.body.append(editor);
    const textNode = editor.firstChild!;
    const range = document.createRange();
    range.setStart(textNode, 0);
    range.setEnd(textNode, 5); // selects "Hello"

    insertHtmlAtRange(editor, range, "Goodbye");

    expect(editor.textContent).toBe("Goodbye world");
  });

  it("inserts into an empty editor", () => {
    const editor = document.createElement("div");
    document.body.append(editor);
    const range = document.createRange();
    range.selectNodeContents(editor);
    range.collapse(false);

    insertHtmlAtRange(editor, range, "Hi there");

    expect(editor.textContent).toBe("Hi there");
  });
});

it("serializes inline attachment previews back to their content IDs", () => {
  const editor = document.createElement("div");
  editor.innerHTML = '<span data-compose-image="true"><img src="data:image/png;base64,aGk=" data-compose-source="cid:image-1@threestrands.local" alt="Screenshot"></span>';

  expect(serializeComposeHtml(editor)).toBe('<img src="cid:image-1@threestrands.local" alt="Screenshot">');
});
