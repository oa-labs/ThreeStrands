/**
 * CHANGELOG.md is the single source for GitHub release notes and the
 * marketing site's changelog page. Agents add plain-language entries under
 * `## Unreleased`; `pnpm release` stamps them with the version being tagged.
 *
 * Format (anything else is rejected so a typo fails CI, not a release):
 *
 *   # Changelog
 *   <preamble>
 *   ## Unreleased
 *   ### New | Improved | Fixed
 *   - entry, optionally continued on lines indented by two spaces
 *   ## 1.2.3 — 2026-10-14          release sections, newest first
 *   ## Highlights: Some Period      summary sections last; never in release notes
 *   <optional intro paragraph>
 */

export const CHANGELOG_GROUPS = ["New", "Improved", "Fixed"];

const number = "(?:0|[1-9][0-9]*)";
const versionPattern = `${number}\\.${number}\\.${number}(?:-beta\\.${number})?`;
const releaseHeading = new RegExp(`^## (${versionPattern}) — (\\d{4}-\\d{2}-\\d{2})$`);
const versionOnly = new RegExp(`^${versionPattern}$`);

/** Orders `MAJOR.MINOR.PATCH[-beta.N]`; a stable version sorts after its betas. */
export function compareVersions(left, right) {
  const parts = (version) => {
    const [core, beta] = version.split("-beta.");
    return [...core.split(".").map(Number), beta === undefined ? Infinity : Number(beta)];
  };
  const a = parts(left);
  const b = parts(right);
  for (let index = 0; index < a.length; index += 1) {
    if (a[index] !== b[index]) return a[index] < b[index] ? -1 : 1;
  }
  return 0;
}

export function parseChangelog(source) {
  const lines = source.replace(/\r\n/g, "\n").split("\n");
  if (lines[0] !== "# Changelog") throw new Error("CHANGELOG.md line 1: must be \"# Changelog\"");
  const fail = (index, message) => {
    throw new Error(`CHANGELOG.md line ${index + 1}: ${message}`);
  };

  let first = lines.findIndex((line) => line.startsWith("## "));
  if (first < 0) first = lines.length;
  const preamble = lines.slice(0, first).join("\n").trimEnd();
  const sections = [];
  let section = null;
  let group = null;
  let item = null;

  for (let index = first; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.startsWith("## ")) {
      const title = line.slice(3).trim();
      const release = releaseHeading.exec(line);
      if (title === "Unreleased") {
        if (sections.length) fail(index, "## Unreleased must be the first section");
        section = { kind: "unreleased", title, intro: [], groups: [] };
      } else if (release) {
        if (sections.some((other) => other.kind === "summary")) fail(index, "release sections must come before summary sections");
        const previous = sections.findLast((other) => other.kind === "release");
        if (previous && compareVersions(previous.version, release[1]) <= 0) {
          fail(index, `${release[1]} must be listed below the newer ${previous.version}`);
        }
        section = { kind: "release", title, version: release[1], date: release[2], intro: [], groups: [] };
      } else if (/^\d/.test(title)) {
        fail(index, `release headings look like "## 1.2.3 — 2026-10-14" (an em dash and an ISO date): ${line}`);
      } else {
        section = { kind: "summary", title, intro: [], groups: [] };
      }
      if (!sections.length && section.kind !== "unreleased") fail(index, "the first section must be ## Unreleased");
      sections.push(section);
      group = null;
      item = null;
    } else if (line.startsWith("### ")) {
      const name = line.slice(4).trim();
      if (!CHANGELOG_GROUPS.includes(name)) fail(index, `groups are ${CHANGELOG_GROUPS.join(", ")}; found "${name}"`);
      if (section.groups.some((other) => other.name === name)) fail(index, `"${name}" appears twice in ${section.title}`);
      group = { name, items: [] };
      section.groups.push(group);
      item = null;
    } else if (line.startsWith("- ")) {
      if (!group) fail(index, "entries belong under a ### New, ### Improved, or ### Fixed group");
      if (!line.slice(2).trim()) fail(index, "empty entry");
      item = { lines: [line] };
      group.items.push(item);
    } else if (/^ {2,}\S/.test(line) && item) {
      item.lines.push(line);
    } else if (line.trim() === "") {
      if (item && lines[index + 1]?.match(/^ {2,}\S/)) fail(index + 1, "an entry's continuation lines cannot follow a blank line");
      item = null;
    } else if (section.kind === "summary" && !group) {
      section.intro.push(line);
    } else {
      fail(index, `expected a ### group or a "- " entry: ${line}`);
    }
  }

  if (!sections.length || sections[0].kind !== "unreleased") {
    throw new Error("CHANGELOG.md needs an ## Unreleased section for upcoming changes");
  }
  for (const each of sections) {
    for (const eachGroup of each.groups) {
      if (!eachGroup.items.length) throw new Error(`CHANGELOG.md: ### ${eachGroup.name} in ${each.title} has no entries`);
      for (const eachItem of eachGroup.items) {
        eachItem.text = eachItem.lines.map((text) => text.trim()).join(" ").slice(2);
      }
    }
  }
  return { preamble, sections };
}

function formatSection(section) {
  const blocks = [`## ${section.title}`];
  if (section.intro.length) blocks.push(section.intro.join("\n").trim());
  for (const name of CHANGELOG_GROUPS) {
    const group = section.groups.find((candidate) => candidate.name === name);
    if (group) blocks.push(`### ${name}\n${group.items.map((item) => item.lines.join("\n")).join("\n")}`);
  }
  return blocks.join("\n\n");
}

export function formatChangelog({ preamble, sections }) {
  return `${[preamble, ...sections.map(formatSection)].join("\n\n")}\n`;
}

/**
 * Moves the Unreleased entries into a section for `version`, merging into an
 * existing section when a tag is re-cut. Returns the source unchanged when
 * there is nothing to stamp.
 */
export function stampRelease(source, version, date) {
  if (!versionOnly.test(version)) throw new Error(`Invalid release version ${version}`);
  const changelog = parseChangelog(source);
  const [unreleased, ...rest] = changelog.sections;
  if (!unreleased.groups.length) return source;

  let target = rest.find((section) => section.kind === "release" && section.version === version);
  if (!target) {
    const newest = rest.find((section) => section.kind === "release");
    if (newest && compareVersions(newest.version, version) > 0) {
      throw new Error(`CHANGELOG.md already lists ${newest.version}, which is newer than ${version}`);
    }
    target = { kind: "release", title: `${version} — ${date}`, version, date, intro: [], groups: [] };
    rest.unshift(target);
  }
  for (const group of unreleased.groups) {
    const existing = target.groups.find((candidate) => candidate.name === group.name);
    if (existing) existing.items.unshift(...group.items);
    else target.groups.push(group);
  }
  unreleased.groups = [];
  return formatChangelog({ preamble: changelog.preamble, sections: [unreleased, ...rest] });
}

/**
 * GitHub release notes for `tag`: every release section newer than the last
 * published release, merged by group. A stable release counts back to the
 * previous stable release so beta users' notes don't hide changes from
 * everyone else. Drafts must already be excluded from `publishedTags`.
 */
export function releaseNotes(source, tag, publishedTags, { repository, siteUrl } = {}) {
  const version = tag.replace(/^v/, "");
  if (!versionOnly.test(version)) throw new Error(`Invalid release tag ${tag}`);
  const stable = !version.includes("-beta.");
  const previous = publishedTags
    .map((candidate) => candidate.trim().replace(/^v/, ""))
    .filter((candidate) => versionOnly.test(candidate) && compareVersions(candidate, version) < 0)
    .filter((candidate) => !stable || !candidate.includes("-beta."))
    .sort(compareVersions)
    .at(-1);

  const included = parseChangelog(source).sections.filter((section) => section.kind === "release"
    && compareVersions(section.version, version) <= 0
    && (previous === undefined || compareVersions(section.version, previous) > 0));
  const blocks = [];
  for (const name of CHANGELOG_GROUPS) {
    const items = included.flatMap((section) => section.groups.find((group) => group.name === name)?.items ?? []);
    if (items.length) blocks.push(`### ${name}\n${items.map((item) => `- ${item.text}`).join("\n")}`);
  }
  if (!blocks.length) blocks.push("Maintenance and reliability improvements.");
  const links = [];
  if (siteUrl) links.push(`[Full release history](${siteUrl.replace(/\/?$/, "/")}changelog.html)`);
  if (previous && repository) links.push(`[Code changes since v${previous}](https://github.com/${repository}/compare/v${previous}...${tag})`);
  if (links.length) blocks.push(links.join(" · "));
  return `${blocks.join("\n\n")}\n`;
}
