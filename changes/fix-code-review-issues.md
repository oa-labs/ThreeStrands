### Fixed
- Inline embedded images (`data:`) in messages now display directly without showing a false "Images are blocked" notice.
- A single failed outbox item no longer stops scheduled sends for other messages or accounts.
- Exporting notes to a folder now applies macOS quarantine and Windows download marks to exported attachments.
- Join codes in **Settings → Sync** are now hidden like passwords by default with a **Show** / **Hide** toggle and state clearly that a code grants access to your sync history.
- IMAP sync no longer forces a full mailbox re-download when a server omits a mailbox identifier, and stuck changes no longer keep an account polling every 15 seconds.
