import { describe, expect, it } from "vitest";
import { attachmentKind } from "./attachmentKind";

describe("attachmentKind", () => {
  it("reads the kind from the extension, whatever its case", () => {
    expect(attachmentKind("Screenshot 2026-09-16 at 11.49.10 AM.png")).toBe("image");
    expect(attachmentKind("Crunchtime errors.XLSX")).toBe("spreadsheet");
    expect(attachmentKind("Q3 review.pptx")).toBe("presentation");
    expect(attachmentKind("Contract.pdf")).toBe("document");
    expect(attachmentKind("export.csv")).toBe("spreadsheet");
    expect(attachmentKind("logs.tar.gz")).toBe("archive");
    expect(attachmentKind("memo.m4a")).toBe("audio");
    expect(attachmentKind("demo.mov")).toBe("video");
    expect(attachmentKind("config.json")).toBe("code");
  });

  it("falls back to the MIME type when the extension is missing or unknown", () => {
    expect(attachmentKind("scan", "image/jpeg")).toBe("image");
    expect(attachmentKind("invoice.bin", "application/pdf; name=invoice")).toBe("document");
    expect(attachmentKind("report", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")).toBe("spreadsheet");
    expect(attachmentKind("deck", "application/vnd.ms-powerpoint")).toBe("presentation");
    expect(attachmentKind("bundle", "application/zip")).toBe("archive");
  });

  it("prefers a known extension over a generic MIME type", () => {
    expect(attachmentKind("photo.heic", "application/octet-stream")).toBe("image");
  });

  it("calls anything it cannot place other", () => {
    expect(attachmentKind("README")).toBe("other");
    expect(attachmentKind("data.xyz", "application/octet-stream")).toBe("other");
    // A leading dot is a hidden file's name, not an extension.
    expect(attachmentKind(".png")).toBe("other");
  });
});
