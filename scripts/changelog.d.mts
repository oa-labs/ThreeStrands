export type ChangelogGroupName = "New" | "Improved" | "Fixed";
export type ChangelogItem = { lines: string[]; text: string };
export type ChangelogGroup = { name: ChangelogGroupName; items: ChangelogItem[] };
export type ChangelogSection =
  | { kind: "unreleased"; title: string; intro: string[]; groups: ChangelogGroup[] }
  | { kind: "release"; title: string; version: string; date: string; intro: string[]; groups: ChangelogGroup[] }
  | { kind: "summary"; title: string; intro: string[]; groups: ChangelogGroup[] };
export type Changelog = { preamble: string; sections: ChangelogSection[] };

export const CHANGELOG_GROUPS: ChangelogGroupName[];
export function compareVersions(left: string, right: string): number;
export function parseChangelog(source: string): Changelog;
export function formatChangelog(changelog: Changelog): string;
export function stampRelease(source: string, version: string, date: string): string;
export function releaseNotes(
  source: string,
  tag: string,
  publishedTags: string[],
  options?: { repository?: string; siteUrl?: string },
): string;
