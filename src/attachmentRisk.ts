/**
 * Why an attachment could harm the computer when opened, judged by its name
 * and type so every sender and provider is treated the same. A file with no
 * risk returns null. This only warns ahead of time: opening goes through the
 * native check in `attachment_security.rs`, which also reads the file's
 * contents and asks before opening anything risky. The extension lists here
 * mirror that module's, and a test keeps them equal.
 */
export type AttachmentRisk = { kind: "program" | "macro" | "diskImage" | "webPage" | "disguised"; reason: string };

export const RISKY_EXTENSIONS: Record<Exclude<AttachmentRisk["kind"], "disguised">, readonly string[]> = {
  // Opened directly, these run code or install software.
  program: [
    "action", "app", "appimage", "appref-ms", "appx", "application", "bat", "bash", "cab", "chm", "cmd", "com",
    "command", "cpl", "csh", "deb", "desktop", "dll", "exe", "fish", "gadget", "hlp", "hta", "inf", "jar", "js",
    "jse", "jnlp", "ksh", "library-ms", "lnk", "msc", "msi", "msix", "msp", "pkg", "pl", "ps1", "psm1", "py", "rb",
    "reg", "rpm", "scf", "scpt", "scr", "settingcontent-ms", "sh", "terminal", "url", "vb", "vbe", "vbs", "workflow",
    "wsf", "wsh", "xll", "zsh",
  ],
  macro: [
    "doc", "docm", "dot", "dotm", "pot", "potm", "ppam", "pps", "ppsm", "ppt", "pptm", "sldm",
    "xla", "xlam", "xls", "xlsb", "xlsm", "xlt", "xltm",
  ],
  diskImage: ["dmg", "img", "iso", "vhd", "vhdx"],
  // A local page can imitate a sign-in form and send what is typed anywhere.
  webPage: ["htm", "html", "mht", "mhtml", "svg", "xhtml"],
};

const REASONS: Record<AttachmentRisk["kind"], string> = {
  program: "Programs, scripts and installers can run code on your computer.",
  macro: "This Office file can contain macros, which can run code on your computer.",
  diskImage: "Disk images can hide programs that run when you open them.",
  webPage: "Attached web pages can imitate sign-in forms to steal passwords.",
  disguised: "This file's name hides its real type, a common way to disguise harmful files.",
};

const KIND_BY_EXTENSION = new Map(Object.entries(RISKY_EXTENSIONS).flatMap(([kind, extensions]) =>
  extensions.map((extension) => [extension, kind as Exclude<AttachmentRisk["kind"], "disguised">])));

/** Types a person expects to be safe to open, which harmful files pose as. */
const DECOY_EXTENSIONS = new Set(["pdf", "docx", "xlsx", "pptx", "txt", "rtf", "jpg", "jpeg", "png", "gif", "zip", "csv"]);

function riskFromMimeType(mimeType: string): Exclude<AttachmentRisk["kind"], "disguised"> | null {
  const type = mimeType.split(";", 1)[0].trim().toLocaleLowerCase();
  if (type === "application/x-apple-diskimage") return "diskImage";
  if (/^application\/(x-msdownload|x-msdos-program|x-ms-installer|x-msi|vnd\.microsoft\.portable-executable|x-dosexec|x-executable|x-sh|x-bat|java-archive|hta)$/.test(type)) return "program";
  if (/macroenabled/.test(type)) return "macro";
  if (/^(text\/html|application\/xhtml\+xml|image\/svg\+xml)$/.test(type)) return "webPage";
  return null;
}

export function attachmentRisk(filename: string, mimeType = ""): AttachmentRisk | null {
  const parts = filename.trim().toLocaleLowerCase().replace(/[\s.]+$/, "").split(".");
  const extension = parts.length > 1 ? parts[parts.length - 1] : "";
  const kind = KIND_BY_EXTENSION.get(extension) ?? riskFromMimeType(mimeType);
  if (!kind) return null;
  // "invoice.pdf.exe": a program dressed as a document.
  if (kind === "program" && parts.length > 2 && DECOY_EXTENSIONS.has(parts[parts.length - 2].trim())) {
    return { kind: "disguised", reason: REASONS.disguised };
  }
  return { kind, reason: REASONS[kind] };
}
