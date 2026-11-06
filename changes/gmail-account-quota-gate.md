### Fixed
- Gmail accounts no longer run into Gmail's per-minute usage limit while syncing, opening messages, and saving drafts at the same time. When Gmail does report the limit, ThreeStrands now pauses that account's requests until the limit resets instead of retrying until sync fails.
  When the same account is signed in on several computers, each one slows down after Gmail reports the limit and speeds back up gradually, so they share the limit instead of taking turns failing.
