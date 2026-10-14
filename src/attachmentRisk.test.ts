import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { attachmentRisk, RISKY_EXTENSIONS } from "./attachmentRisk";

/** A `const NAME: &[&str] = &[...]` list from the native attachment check. */
function nativeList(name: string): string[] {
  const source = readFileSync(resolve(process.cwd(), "src-tauri/src/attachment_security.rs"), "utf8");
  const body = new RegExp(`const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`).exec(source)?.[1];
  if (!body) throw new Error(`${name} not found`);
  return [...body.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

describe("attachmentRisk", () => {
  it("warns about exactly the extensions the native open check confirms", () => {
    // The warning shown before opening must match the prompt shown when opening.
    const native = [...nativeList("DANGEROUS_EXTENSIONS"), ...nativeList("MACRO_EXTENSIONS")].sort();
    const warned = Object.values(RISKY_EXTENSIONS).flat().sort();
    expect(warned).toEqual(native);
    expect(new Set(RISKY_EXTENSIONS.macro)).toEqual(new Set(nativeList("MACRO_EXTENSIONS")));
  });

  it("flags programs, macro documents, disk images and web pages by extension, in any case", () => {
    expect(attachmentRisk("setup.EXE")?.kind).toBe("program");
    expect(attachmentRisk("run.command")?.kind).toBe("program");
    expect(attachmentRisk("budget.xlsm")?.kind).toBe("macro");
    expect(attachmentRisk("Letter.doc")?.kind).toBe("macro");
    expect(attachmentRisk("installer.iso")?.kind).toBe("diskImage");
    expect(attachmentRisk("App.dmg")?.kind).toBe("diskImage");
    expect(attachmentRisk("Secure Message.html")?.kind).toBe("webPage");
    expect(attachmentRisk("drawing.svg")?.kind).toBe("webPage");
  });

  it("flags a risky type from its MIME type when the name has no telling extension", () => {
    expect(attachmentRisk("payload", "application/x-msdownload")?.kind).toBe("program");
    expect(attachmentRisk("statement", "text/html; charset=utf-8")?.kind).toBe("webPage");
    expect(attachmentRisk("sheet", "application/vnd.ms-excel.sheet.macroEnabled.12")?.kind).toBe("macro");
    expect(attachmentRisk("disk", "application/x-apple-diskimage")?.kind).toBe("diskImage");
  });

  it("names a program posing as a document as disguised", () => {
    expect(attachmentRisk("invoice.pdf.exe")?.kind).toBe("disguised");
    expect(attachmentRisk("photo.JPG .scr")?.kind).toBe("disguised");
    expect(attachmentRisk("notes.v2.exe")?.kind).toBe("program");
  });

  it("ignores trailing dots and spaces that would otherwise hide the extension", () => {
    expect(attachmentRisk("update.exe. ")?.kind).toBe("program");
    expect(attachmentRisk("update.exe...")?.kind).toBe("program");
  });

  it("leaves ordinary documents, images, archives and extensionless files alone", () => {
    for (const [name, type] of [
      ["Slides Intro to EO (1).pdf", "application/pdf"],
      ["notes.v2.txt", "text/plain"],
      ["forecast.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"],
      ["Joel.png", "image/png"],
      ["photos.zip", "application/zip"],
      ["README", "application/octet-stream"],
      [".htaccess", ""],
    ]) expect(attachmentRisk(name, type), name).toBeNull();
  });

  it("explains each risk in plain language", () => {
    expect(attachmentRisk("setup.exe")?.reason).toBe("Programs, scripts and installers can run code on your computer.");
    expect(attachmentRisk("invoice.pdf.exe")?.reason).toMatch(/hides its real type/);
  });
});
