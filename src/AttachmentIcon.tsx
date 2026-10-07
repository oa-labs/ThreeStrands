import { File, FileArchive, FileAudio, FileCode, FileImage, FileSpreadsheet, FileText, FileVideo, Presentation, type LucideIcon } from "lucide-react";
import { attachmentKind, type AttachmentKind } from "./attachmentKind";
import { ICON_SIZE } from "./iconSizes";

const ICONS: Record<AttachmentKind, LucideIcon> = {
  image: FileImage,
  spreadsheet: FileSpreadsheet,
  presentation: Presentation,
  document: FileText,
  archive: FileArchive,
  audio: FileAudio,
  video: FileVideo,
  code: FileCode,
  other: File,
};

/** A file's icon by its kind, so a glance tells an image from a spreadsheet. Decorative: the filename names it. */
export function AttachmentIcon({ filename, mimeType, size }: { filename: string; mimeType?: string; size: keyof typeof ICON_SIZE }) {
  const kind = attachmentKind(filename, mimeType);
  const Icon = ICONS[kind];
  return <Icon size={ICON_SIZE[size]} aria-hidden="true" data-attachment-kind={kind} />;
}
