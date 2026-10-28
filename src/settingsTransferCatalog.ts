/**
 * What the encrypted settings file carries, keyed by the export payload's
 * field in `src-tauri/src/transfer.rs`. A test keeps this list in step with
 * that payload, so the hint never understates what leaves the computer.
 */
export const TRANSFERRED_DATA = [
  { field: "preferences", label: "preferences", replacedOnImport: true },
  { field: "accounts", label: "account list", replacedOnImport: false },
  { field: "split_inboxes", label: "Split Inboxes", replacedOnImport: true },
  { field: "signatures", label: "signatures and their defaults", replacedOnImport: true },
  { field: "snippets", label: "snippets", replacedOnImport: true },
  { field: "contacts", label: "saved contacts", replacedOnImport: true },
  { field: "contact_groups", label: "contact groups", replacedOnImport: true },
  { field: "sender_rules", label: "muted senders", replacedOnImport: true },
  { field: "retention_days", label: "retention setting", replacedOnImport: true },
] as const;

