# Archived plans

Completed plans kept for design history. They describe the code as it was when
each plan was written; the code and the documents in [`docs/`](../) are the
current reference.

| Document | Outcome |
| --- | --- |
| [multi-account.md](multi-account.md) | Multiple Gmail accounts and the unified inbox, implemented 2026-09-12 to 2026-09-16. |
| [phase-2-compose.md](phase-2-compose.md) | Compose, drafts, send/undo, and attachments. Current behavior is in [features.md](../features.md). |
| [db-recovery-plan.md](db-recovery-plan.md) | Durability pragmas, checkpoints, backups, and startup recovery, implemented 2026-09-17. |
| [db-modularization-plan.md](db-modularization-plan.md) | Split of `db.rs` into `db/` modules, implemented in 0.86.2. |
| [imap-spike-findings.md](imap-spike-findings.md) | The `async-imap` 0.12 gate for IMAP phase 2, passed 2026-10-09. |
| [workspace-context-and-files-plan.md](workspace-context-and-files-plan.md) | Task and calendar context rails, then the Files workspace, implemented in 0.111.0. Current behavior is in [features.md](../features.md). |
| [db-layer-plan.md](db-layer-plan.md) | Moving SQL back into `db/` and making the compiler enforce that boundary, completed in 0.173.8. |
