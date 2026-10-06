import { afterEach, describe, expect, it, vi } from "vitest";
import { sanitizeMessageHtml } from "./SafeMessage";
import { collapseQuotedHistoryHtml } from "./quotedHistory";
import {
  applyAsteriskListShortcut,
  composeHtmlToText,
  draftTextToComposeHtml,
  applyFormattingShortcut,
  formattingShortcutFor,
  formattingShortcuts,
  insertHtmlAtRange,
  plainTextToHtml,
  sanitizeComposeHtml,
  serializeComposeBody,
  serializeComposeHtml,
  splitReplyQuote,
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

it("derives the draft's HTML and plain text from a single clone without image controls", () => {
  const editor = document.createElement("div");
  editor.innerHTML = 'Before <span data-compose-image="true"><img src="data:image/png;base64,aGk=" data-compose-source="cid:image-1@threestrands.local" alt="Screenshot"><button data-compose-image-remove="true">×</button><span data-compose-image-resize="true"></span></span> after<br>&gt; quoted';
  const cloneNode = vi.spyOn(editor, "cloneNode");

  const { html, text } = serializeComposeBody(editor);

  expect(cloneNode).toHaveBeenCalledTimes(1);
  expect(html).toBe('Before <img src="cid:image-1@threestrands.local" alt="Screenshot"> after<br>&gt; quoted');
  expect(text).toBe("Before  after\n> quoted");
  expect(text).not.toContain("×");
});

describe("reply citations", () => {
  const reply = "\n\nOn Mon, Oct 5, 2026 at 10:32 AM, A. Sender <sender@example.com> wrote:\n> Is this still happening?\n> > Earlier thread line";
  const editorFor = (html: string) => {
    const editor = document.createElement("div");
    editor.innerHTML = sanitizeComposeHtml(html);
    return editor;
  };

  it("turns a native reply body into a nested citation under its attribution", () => {
    const editor = editorFor(draftTextToComposeHtml(reply));
    const citation = editor.querySelector(':scope > blockquote[type="cite"]');
    expect(editor.innerHTML.startsWith("<br><br>On Mon, Oct 5, 2026 at 10:32 AM, A. Sender &lt;sender@example.com&gt; wrote:")).toBe(true);
    expect(citation?.firstChild?.textContent).toBe("Is this still happening?");
    expect(citation?.querySelector('blockquote[type="cite"]')).toHaveTextContent("Earlier thread line");
    expect(citation?.getAttribute("style")).toContain("border-left");
  });

  it("round-trips a reply body through the editor to the same quoted text", () => {
    expect(composeHtmlToText(editorFor(draftTextToComposeHtml(reply)))).toBe(reply);
    expect(composeHtmlToText(editorFor(draftTextToComposeHtml("\n\nOn Mon, A wrote:\n> one\n> \n> two"))))
      .toBe("\n\nOn Mon, A wrote:\n> one\n>\n> two");
  });

  it("keeps quoted markup as text rather than HTML", () => {
    const editor = editorFor(draftTextToComposeHtml('\n\nOn Mon, A wrote:\n> <img src=x onerror="alert(1)"><script>alert(2)</script>'));
    expect(editor.querySelector("img, script")).toBeNull();
    expect(editor.querySelector("blockquote")).toHaveTextContent('<img src=x onerror="alert(1)"><script>alert(2)</script>');
  });

  it("leaves non-reply text and replies followed by unquoted text as plain lines", () => {
    expect(draftTextToComposeHtml("Hello\n> not a reply")).toBe("Hello<br>&gt; not a reply");
    expect(draftTextToComposeHtml("On Mon, A wrote:\n> quoted\nmy inline answer")).not.toContain("blockquote");
    expect(draftTextToComposeHtml("\n\nOn Mon, A wrote:\n")).not.toContain("blockquote");
  });

  it("allows type only as a blockquote citation and replaces a citation's own style", () => {
    const html = sanitizeComposeHtml('<div type="cite">a</div><blockquote type="text/javascript">b</blockquote><blockquote type="CITE" style="background-image:url(https://tracker.invalid);position:fixed">c</blockquote>');
    const container = document.createElement("div");
    container.innerHTML = html;
    expect(container.querySelector("div")?.hasAttribute("type")).toBe(false);
    expect(container.querySelectorAll("blockquote")[0].hasAttribute("type")).toBe(false);
    expect(container.querySelectorAll("blockquote")[1].getAttribute("type")).toBe("cite");
    expect(html).not.toMatch(/url\(|position|tracker/);
    expect(sanitizeComposeHtml(html)).toBe(html);
  });

  it("serializes editor blocks, line breaks and nested quotes as a plain-text alternative", () => {
    const editor = document.createElement("div");
    editor.innerHTML = "Hi<div>there</div><div><br></div><div>a&nbsp;b</div><blockquote>q1<br>q2<blockquote><div>deep</div></blockquote></blockquote><div>after</div>";
    expect(composeHtmlToText(editor)).toBe("Hi\nthere\n\na b\n> q1\n> q2\n> > deep\nafter");
  });

  it("produces a reply the reader folds at its attribution", () => {
    const editor = editorFor(`Friday works.${draftTextToComposeHtml(reply)}`);
    const folded = collapseQuotedHistoryHtml(sanitizeMessageHtml(serializeComposeBody(editor).html));
    expect(folded).toContain("Friday works.");
    expect(folded).not.toMatch(/wrote:|Is this still happening/);
  });
});

describe("reply quote split", () => {
  const reply = "\n\nOn Mon, Oct 5, 2026 at 10:32 AM, A. Sender <sender@example.com> wrote:\n> Is this still happening?\n> > Earlier thread line";
  const nativeHtml = () => sanitizeComposeHtml(draftTextToComposeHtml(reply));
  const editorFor = (html: string) => {
    const editor = document.createElement("div");
    editor.innerHTML = html;
    return editor;
  };

  it("separates a native reply's attribution and citation from the authored text", () => {
    const split = splitReplyQuote(sanitizeComposeHtml(`Friday works.${draftTextToComposeHtml(reply)}`));
    expect(split?.authoredHtml).toBe("Friday works.");
    expect(split?.quotedHtml.startsWith("On Mon, Oct 5, 2026 at 10:32 AM, A. Sender &lt;sender@example.com&gt; wrote:<blockquote type=\"cite\"")).toBe(true);
    expect(splitReplyQuote(nativeHtml())?.authoredHtml).toBe("");
  });

  it("splits an edited reply whose lines and attribution were wrapped in blocks", () => {
    const html = '<div>Friday works.</div><div><br></div><div>On Mon, A wrote:</div><blockquote type="cite">one<br>two</blockquote><br>';
    const split = splitReplyQuote(html);
    expect(split?.authoredHtml).toBe("<div>Friday works.</div><div><br></div>");
    expect(split?.quotedHtml).toBe('<div>On Mon, A wrote:</div><blockquote type="cite">one<br>two</blockquote><br>');
  });

  it("leaves a body whole without conservative evidence of a trailing reply quote", () => {
    expect(splitReplyQuote("Just a note")).toBeNull();
    expect(splitReplyQuote('Hi<br><blockquote type="cite">no attribution</blockquote>')).toBeNull();
    expect(splitReplyQuote("Hi<br>On Mon, A wrote:<blockquote>not a citation</blockquote>")).toBeNull();
    expect(splitReplyQuote('On Mon, A wrote:<blockquote type="cite">q</blockquote>My inline answer')).toBeNull();
    expect(splitReplyQuote('<div>Per our call</div><blockquote type="cite">q</blockquote>')).toBeNull();
  });

  it("rejoins the authored text and quoted history into the native reply shape", () => {
    const split = splitReplyQuote(nativeHtml())!;
    const editor = editorFor("Friday works.<br><br>");
    const { html, text } = serializeComposeBody(editor, editorFor(split.quotedHtml));
    expect(text).toBe(`Friday works.${reply}`);
    expect(splitReplyQuote(html)).toEqual({ authoredHtml: "Friday works.", quotedHtml: split.quotedHtml });
    expect(serializeComposeBody(editorFor(""), editorFor(split.quotedHtml)).html).toBe(nativeHtml());
  });

  it("drops the separator when the quoted history was deleted", () => {
    expect(serializeComposeBody(editorFor("Friday works."), editorFor("")).text).toBe("Friday works.");
  });
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
