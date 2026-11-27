# Crash reporting policy

Crash reporting is disabled by default and is never required to use ThreeStrands.
The user enables or disables it from Settings → Diagnostics.

When enabled, the client retains at most 20 sanitized reports locally. A build
may set `VITE_CRASH_REPORT_ENDPOINT` to submit the same reports to a project
operator. Official builds must identify that operator and publish its retention
period before setting an endpoint. A Tauri build must also add that exact
endpoint origin to its Content Security Policy; the default policy permits no
external reporting destination.

Reports contain:

- a random report ID and timestamp;
- error or unhandled-rejection category (covering `window` `error`,
  `unhandledrejection`, and React `ErrorBoundary` catches via `reportCaughtError`);
- sanitized error message and stack;
- application version; and
- browser/webview user-agent string.

Email addresses, URLs, single- and double-quoted strings, and header-style
lines (`subject|to|cc|bcc|from:`) are redacted, and messages/stacks are bounded
to 4,000 characters (`src/crashReporting.ts`). Reports must never intentionally
contain message bodies, subjects, recipients, OAuth tokens, search terms,
attachment names, or SQLite contents. Redacted warnings/errors are always written
to the device-local Tauri log (`writeNativeLog`), which stays on the device even
when crash reporting is off. Reporting failures are retained locally and never
interrupt an email action. Disabling reporting stops collection; clearing
locally retained reports is a separate explicit action.
