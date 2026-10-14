import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { describe, it } from "node:test";
import { compareVersions, formatChangelog, parseChangelog, releaseNotes, stampRelease } from "./changelog.mjs";

const changelog = (...sections) => `# Changelog\n\nWhat changed.\n\n${["## Unreleased", ...sections].join("\n\n")}\n`;

const history = changelog(
  "## 0.3.0 — 2026-10-03\n\n### New\n- Third feature.\n\n### Fixed\n- Third fix.",
  "## 0.3.0-beta.1 — 2026-10-02\n\n### New\n- Beta feature.",
  "## 0.2.0 — 2026-10-02\n\n### Improved\n- Second improvement.",
  "## 0.1.0 — 2026-10-01\n\n### New\n- First feature,\n  continued on a second line.",
  "## Highlights: Early Days\n\nAn overview.\n\n### New\n- Summary entry.",
);

describe("changelog format", () => {
  it("keeps the repository changelog parseable and canonically formatted", async () => {
    const source = await readFile(new URL("../CHANGELOG.md", import.meta.url), "utf8");
    assert.equal(formatChangelog(parseChangelog(source)), source, "Run formatChangelog(parseChangelog(...)) to see the canonical layout");
  });

  it("parses unreleased, release, and summary sections with joined continuation lines", () => {
    const { sections } = parseChangelog(history);
    assert.deepEqual(sections.map((section) => [section.kind, section.version ?? section.title]), [
      ["unreleased", "Unreleased"], ["release", "0.3.0"], ["release", "0.3.0-beta.1"], ["release", "0.2.0"],
      ["release", "0.1.0"], ["summary", "Highlights: Early Days"],
    ]);
    assert.equal(sections[4].groups[0].items[0].text, "First feature, continued on a second line.");
    assert.deepEqual(sections[5].intro, ["An overview."]);
    assert.equal(formatChangelog(parseChangelog(history)), history);
  });

  for (const [problem, source, message] of [
    ["a missing title", "## Unreleased\n", /# Changelog/],
    ["a missing Unreleased section", changelog().replace("## Unreleased", "## 0.1.0 — 2026-10-01"), /first section must be ## Unreleased/],
    ["a second Unreleased section", changelog("## Unreleased"), /must be the first section/],
    ["a malformed release heading", changelog("## 0.1.0 - 2026-10-01"), /em dash/],
    ["a heading without a date", changelog("## 0.1.0"), /em dash and an ISO date/],
    ["an unknown group", changelog("## 0.1.0 — 2026-10-01\n\n### Changed\n- x"), /groups are New, Improved, Fixed/],
    ["a repeated group", changelog("## 0.1.0 — 2026-10-01\n\n### New\n- x\n\n### New\n- y"), /appears twice/],
    ["an empty group", changelog("## 0.1.0 — 2026-10-01\n\n### New"), /has no entries/],
    ["an entry outside a group", changelog("## 0.1.0 — 2026-10-01\n\n- x"), /belong under/],
    ["prose in a release section", changelog("## 0.1.0 — 2026-10-01\n\nSome text."), /expected a ### group/],
    ["an empty entry", changelog("## 0.1.0 — 2026-10-01\n\n### New\n- "), /empty entry/],
    ["a continuation after a blank line", changelog("## 0.1.0 — 2026-10-01\n\n### New\n- x\n\n  y"), /continuation lines/],
    ["releases out of order", changelog("## 0.1.0 — 2026-10-01\n\n### New\n- x", "## 0.2.0 — 2026-10-02\n\n### New\n- y"), /below the newer 0\.1\.0/],
    ["a duplicate release", changelog("## 0.1.0 — 2026-10-01\n\n### New\n- x", "## 0.1.0 — 2026-10-01\n\n### New\n- y"), /below the newer/],
    ["a release after a summary", changelog("## Highlights\n\n### New\n- x", "## 0.1.0 — 2026-10-01\n\n### New\n- y"), /before summary sections/],
  ]) {
    it(`rejects ${problem} with its line number`, () => {
      assert.throws(() => parseChangelog(source), message);
    });
  }

  it("orders stable versions after their betas and compares numerically", () => {
    const sorted = ["0.10.0", "0.9.1", "0.10.0-beta.2", "0.10.0-beta.10", "1.0.0"].sort(compareVersions);
    assert.deepEqual(sorted, ["0.9.1", "0.10.0-beta.2", "0.10.0-beta.10", "0.10.0", "1.0.0"]);
  });
});

describe("stamping a release", () => {
  it("moves Unreleased entries into a dated section for the version and leaves Unreleased empty", () => {
    // changelog()'s first argument lands directly under ## Unreleased.
    const source = changelog("### New\n- Snooze.\n\n### Fixed\n- A crash.", "## 0.1.0 — 2026-10-01\n\n### New\n- First feature.");
    const stamped = stampRelease(source, "0.2.0", "2026-10-14");
    assert.equal(stamped, changelog(
      "## 0.2.0 — 2026-10-14\n\n### New\n- Snooze.\n\n### Fixed\n- A crash.",
      "## 0.1.0 — 2026-10-01\n\n### New\n- First feature.",
    ));
    assert.equal(parseChangelog(stamped).sections[0].groups.length, 0);
  });

  it("returns the source unchanged when nothing is unreleased", () => {
    const source = changelog("## 0.1.0 — 2026-10-01\n\n### New\n- First feature.");
    assert.equal(stampRelease(source, "0.2.0", "2026-10-14"), source);
  });

  it("merges into the existing section when the same version is tagged again, newest entries first", () => {
    const source = changelog("### New\n- Later feature.\n\n### Fixed\n- Later fix.", "## 0.2.0 — 2026-10-13\n\n### New\n- Earlier feature.");
    assert.equal(stampRelease(source, "0.2.0", "2026-10-14"), changelog(
      "## 0.2.0 — 2026-10-13\n\n### New\n- Later feature.\n- Earlier feature.\n\n### Fixed\n- Later fix.",
    ));
  });

  it("refuses to stamp a version older than one already listed", () => {
    const source = changelog("### New\n- Snooze.", "## 0.3.0 — 2026-10-13\n\n### New\n- Newer.");
    assert.throws(() => stampRelease(source, "0.2.0", "2026-10-14"), /newer than 0\.2\.0/);
  });
});

describe("release notes", () => {
  const options = { repository: "oa-labs/ThreeStrands", siteUrl: "https://example.com" };

  it("merges every section since the last published stable release by group, newest first", () => {
    assert.equal(releaseNotes(history, "v0.3.0", ["v0.1.0", "v0.3.0-beta.1"], options), [
      "### New\n- Third feature.\n- Beta feature.",
      "### Improved\n- Second improvement.",
      "### Fixed\n- Third fix.",
      "[Full release history](https://example.com/changelog.html) · [Code changes since v0.1.0](https://github.com/oa-labs/ThreeStrands/compare/v0.1.0...v0.3.0)",
    ].join("\n\n") + "\n");
  });

  it("counts a beta from the previous published release of either channel", () => {
    const notes = releaseNotes(history, "v0.3.0-beta.1", ["v0.1.0", "v0.2.0"], options);
    assert.match(notes, /^### New\n- Beta feature\.\n\n/);
    assert.doesNotMatch(notes, /Second improvement/);
    assert.match(notes, /compare\/v0\.2\.0\.\.\.v0\.3\.0-beta\.1/);
  });

  it("covers the whole history, without summaries, when nothing has been published", () => {
    const notes = releaseNotes(history, "v0.2.0", [], {});
    assert.equal(notes, "### New\n- First feature, continued on a second line.\n\n### Improved\n- Second improvement.\n");
  });

  it("ignores published releases newer than the tag and malformed tags", () => {
    const notes = releaseNotes(history, "v0.2.0", ["v0.3.0", "nightly", "v0.1.0"], options);
    assert.match(notes, /^### Improved\n- Second improvement\.\n\n/);
    assert.match(notes, /compare\/v0\.1\.0\.\.\.v0\.2\.0/);
  });

  it("says maintenance when no section has entries, and still links the code changes", () => {
    const notes = releaseNotes(changelog("## 0.1.0 — 2026-10-01\n\n### New\n- First."), "v0.1.1", ["v0.1.0"], options);
    assert.match(notes, /^Maintenance and reliability improvements\.\n\n/);
    assert.match(notes, /compare\/v0\.1\.0\.\.\.v0\.1\.1/);
  });

  it("rejects a malformed tag", () => {
    assert.throws(() => releaseNotes(history, "latest", [], options), /Invalid release tag/);
  });
});
