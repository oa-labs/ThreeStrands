# Pending changelog entries

Each change that users can notice adds its release notes here as its own
file, named after its branch (for example `starred-inbox-tab.md`). Because
every branch adds a new file instead of editing `CHANGELOG.md`, branches made
at the same time don't conflict.

A file holds groups and entries exactly as they will appear in
`CHANGELOG.md`, and nothing else:

```markdown
### New
- Star a conversation to keep it in the **Starred** inbox tab.

### Fixed
- Pressing <kbd>Tab</kbd> after choosing recipients now moves to the next field.
```

The groups are `### New`, `### Improved`, and `### Fixed`, in that order.
An entry can continue on lines indented by two spaces.

At release time, `pnpm release` moves these entries into `CHANGELOG.md`,
in file-name order, and deletes the files. Any **New** entry makes the
release a minor version; otherwise it is a patch. See
[Releases](../DEVELOPERS.md#releases).

Check the files with `node --test scripts/changelog.test.mjs`. This README is
not an entry.
