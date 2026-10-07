/** The broad kind of an attachment, which picks its icon. */
export type AttachmentKind = "image" | "spreadsheet" | "presentation" | "document" | "archive" | "audio" | "video" | "code" | "other";

const EXTENSIONS: Record<Exclude<AttachmentKind, "other">, string[]> = {
  image: ["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "bmp", "tif", "tiff", "svg", "avif"],
  spreadsheet: ["xls", "xlsx", "xlsm", "csv", "tsv", "ods", "numbers"],
  presentation: ["ppt", "pptx", "key", "odp"],
  document: ["pdf", "doc", "docx", "odt", "rtf", "txt", "md", "pages"],
  archive: ["zip", "gz", "tgz", "rar", "7z", "tar", "bz2", "xz"],
  audio: ["mp3", "m4a", "wav", "aac", "flac", "ogg", "opus"],
  video: ["mp4", "mov", "m4v", "avi", "mkv", "webm"],
  code: ["json", "xml", "html", "htm", "js", "ts", "css", "py", "yml", "yaml", "sql", "sh"],
};

const KIND_BY_EXTENSION = new Map(Object.entries(EXTENSIONS).flatMap(([kind, extensions]) => extensions.map((extension) => [extension, kind as AttachmentKind])));

function kindFromMimeType(mimeType: string): AttachmentKind {
  const type = mimeType.split(";", 1)[0].trim().toLocaleLowerCase();
  if (type.startsWith("image/")) return "image";
  if (type.startsWith("audio/")) return "audio";
  if (type.startsWith("video/")) return "video";
  if (/spreadsheet|excel|^text\/(csv|tab-separated-values)$/.test(type)) return "spreadsheet";
  if (/presentation|powerpoint/.test(type)) return "presentation";
  if (/^application\/pdf$|msword|wordprocessing|opendocument\.text|^text\/(plain|markdown|rtf)$|^application\/rtf$/.test(type)) return "document";
  if (/zip|gzip|x-7z|x-rar|x-tar|x-bzip|x-xz/.test(type)) return "archive";
  if (/json|xml|javascript|^text\/(html|css)$/.test(type)) return "code";
  return "other";
}

/**
 * An attachment's kind from its extension, falling back to its MIME type,
 * since mail often labels files with a generic `application/octet-stream`.
 */
export function attachmentKind(filename: string, mimeType = ""): AttachmentKind {
  const dot = filename.lastIndexOf(".");
  const extension = dot > 0 ? filename.slice(dot + 1).toLocaleLowerCase() : "";
  return KIND_BY_EXTENSION.get(extension) ?? kindFromMimeType(mimeType);
}
