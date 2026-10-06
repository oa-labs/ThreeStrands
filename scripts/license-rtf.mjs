import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

// The macOS DMG shows the bundle license file in its agreement dialog, which
// renders RTF but shows Markdown verbatim. LICENSE stays the Markdown source
// of truth; src-tauri/LICENSE.rtf is generated from it for the installer.
export const LICENSE_SOURCE = new URL("../LICENSE", import.meta.url);
export const LICENSE_RTF = new URL("../src-tauri/LICENSE.rtf", import.meta.url);

function escapeRtf(text) {
  let out = "";
  for (const char of text) {
    const code = char.codePointAt(0);
    if (char === "\\" || char === "{" || char === "}") out += `\\${char}`;
    else if (code < 0x80) out += char;
    else if (code <= 0xffff) out += `\\u${code > 0x7fff ? code - 0x10000 : code}?`;
    else {
      const high = 0xd800 + ((code - 0x10000) >> 10);
      const low = 0xdc00 + ((code - 0x10000) & 0x3ff);
      out += `\\u${high - 0x10000}?\\u${low - 0x10000}?`;
    }
  }
  return out;
}

function inlineToRtf(text) {
  const pattern = /\*\*\*(.+?)\*\*\*|\*\*(.+?)\*\*|`([^`]+)`|\[([^\]]+)\]\([^)]+\)|<(https?:[^>]+)>/g;
  let out = "";
  let last = 0;
  for (const match of text.matchAll(pattern)) {
    out += escapeRtf(text.slice(last, match.index));
    const [, boldItalic, bold, code, linkText, url] = match;
    if (boldItalic !== undefined) out += `{\\b\\i ${inlineToRtf(boldItalic)}}`;
    else if (bold !== undefined) out += `{\\b ${inlineToRtf(bold)}}`;
    else if (code !== undefined) out += `{\\f1 ${escapeRtf(code)}}`;
    else if (linkText !== undefined) out += inlineToRtf(linkText);
    else out += escapeRtf(url);
    last = match.index + match[0].length;
  }
  return out + escapeRtf(text.slice(last));
}

export function markdownToRtf(markdown) {
  const blocks = markdown.replace(/\r\n?/g, "\n").trim().split(/\n\s*\n/);
  const paragraphs = blocks.map((block) => {
    const lines = block.split("\n").map((line) => line.trim());
    const heading = /^(#{1,6})\s+(.*)$/.exec(lines[0]);
    if (heading && lines.length === 1) {
      const size = heading[1].length === 1 ? 32 : 26;
      return `\\pard\\sb240\\sa120\\keepn\\b\\fs${size} ${inlineToRtf(heading[2])}\\b0\\fs22\\par`;
    }
    if (lines.every((line) => line.startsWith(">"))) {
      const text = lines.map((line) => line.replace(/^>\s?/, "")).join(" ");
      return `\\pard\\li360\\sa180 {\\i ${inlineToRtf(text)}}\\par`;
    }
    return `\\pard\\sa180 ${inlineToRtf(lines.join(" "))}\\par`;
  });
  return [
    "{\\rtf1\\ansi\\ansicpg1252\\deff0",
    "{\\fonttbl{\\f0\\fswiss Helvetica;}{\\f1\\fmodern Menlo;}}",
    "\\f0\\fs22",
    ...paragraphs,
    "}",
    "",
  ].join("\n");
}

export async function renderLicenseRtf() {
  return markdownToRtf(await readFile(LICENSE_SOURCE, "utf8"));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await writeFile(LICENSE_RTF, await renderLicenseRtf());
}
