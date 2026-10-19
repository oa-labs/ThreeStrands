/**
 * Look-alike domains: a site that reads as one the user corresponds with but
 * is a different registration. Compared only against the user's own
 * correspondents and saved contacts, never a list of brands, so every sender
 * is judged the same way. Everything here is local string work.
 */

/** Known names shorter than this are too short to judge a one-letter typo against. */
export const LOOKALIKE_MIN_TYPO_LENGTH = 5;

const BASE = 36;
const T_MIN = 1;
const T_MAX = 26;
const SKEW = 38;
const DAMP = 700;
const INITIAL_BIAS = 72;
const INITIAL_N = 128;

function adapt(delta: number, points: number, first: boolean): number {
  let value = first ? Math.floor(delta / DAMP) : delta >> 1;
  value += Math.floor(value / points);
  let k = 0;
  while (value > ((BASE - T_MIN) * T_MAX) >> 1) {
    value = Math.floor(value / (BASE - T_MIN));
    k += BASE;
  }
  return k + Math.floor(((BASE - T_MIN + 1) * value) / (value + SKEW));
}

function digitOf(code: number): number {
  if (code >= 48 && code <= 57) return code - 22;
  if (code >= 65 && code <= 90) return code - 65;
  if (code >= 97 && code <= 122) return code - 97;
  return BASE;
}

/** Decodes one Punycode label body (RFC 3492), or null when it is malformed. */
function decodePunycode(input: string): string | null {
  const output: number[] = [];
  const basicEnd = input.lastIndexOf("-");
  for (let index = 0; index < Math.max(basicEnd, 0); index += 1) {
    const code = input.charCodeAt(index);
    if (code >= 0x80) return null;
    output.push(code);
  }
  let n = INITIAL_N;
  let bias = INITIAL_BIAS;
  let i = 0;
  for (let position = basicEnd > 0 ? basicEnd + 1 : 0; position < input.length;) {
    const previous = i;
    for (let weight = 1, k = BASE; ; k += BASE) {
      if (position >= input.length) return null;
      const digit = digitOf(input.charCodeAt(position++));
      if (digit >= BASE) return null;
      i += digit * weight;
      const threshold = k <= bias ? T_MIN : k >= bias + T_MAX ? T_MAX : k - bias;
      if (digit < threshold) break;
      weight *= BASE - threshold;
      if (!Number.isSafeInteger(i) || !Number.isSafeInteger(weight)) return null;
    }
    bias = adapt(i - previous, output.length + 1, previous === 0);
    n += Math.floor(i / (output.length + 1));
    i %= output.length + 1;
    if (n > 0x10ffff) return null;
    output.splice(i, 0, n);
    i += 1;
  }
  return String.fromCodePoint(...output);
}

/** A host as people read it: each `xn--` label decoded, lowercased. */
export function unicodeHost(host: string): string {
  return host.toLocaleLowerCase().split(".").map((label) => {
    if (!label.startsWith("xn--")) return label;
    return decodePunycode(label.slice(4)) ?? label;
  }).join(".");
}

/**
 * Letters from other alphabets drawn like Latin ones, and Latin letters drawn
 * like others. Fullwidth and stylized forms are folded by NFKC first, and
 * accents are dropped, so only the shapes NFKC keeps apart are listed.
 */
const CONFUSABLE: Record<string, string> = {
  // Cyrillic
  "а": "a", "в": "b", "е": "e", "ё": "e", "һ": "h", "і": "i", "ї": "i", "ј": "j", "к": "k", "ӏ": "l",
  "м": "m", "н": "h", "о": "o", "р": "p", "с": "c", "ѕ": "s", "т": "t", "у": "y", "х": "x", "ԁ": "d",
  "ԛ": "q", "ԝ": "w", "ь": "b", "ꮃ": "w",
  // Greek
  "α": "a", "β": "b", "ε": "e", "η": "n", "ι": "i", "κ": "k", "ν": "v", "ο": "o", "ρ": "p", "τ": "t",
  "υ": "u", "χ": "x", "γ": "y", "ω": "w",
  // Latin and symbols
  "ı": "i", "ɑ": "a", "ɡ": "g", "ℓ": "l", "ł": "l", "ø": "o", "đ": "d", "ħ": "h",
  // Digits read as letters
  "0": "o", "1": "l", "3": "e", "5": "s",
};

/**
 * A label folded to what it looks like, so two labels that read the same
 * fold to the same string: `examp1e`, `exаmple` (Cyrillic а), and `example`
 * all fold to `example`. `i` and `l` fold together, as do `rn` and `m`, `vv`
 * and `w`.
 */
export function skeleton(value: string): string {
  const folded = value.normalize("NFKC").toLocaleLowerCase().normalize("NFD").replace(/\p{M}/gu, "");
  return [...folded].map((character) => CONFUSABLE[character] ?? character).join("")
    .replace(/rn/g, "m").replace(/vv/g, "w").replace(/i/g, "l");
}

const SCRIPTS = [/\p{Script=Latin}/u, /\p{Script=Cyrillic}/u, /\p{Script=Greek}/u, /\p{Script=Armenian}/u, /\p{Script=Cherokee}/u];
const SCRIPT_NAMES = ["Latin", "Cyrillic", "Greek", "Armenian", "Cherokee"];

/**
 * Whether a label mixes letters from more than one of the alphabets that
 * imitate each other. A real name is written in one alphabet; mixing them
 * in one label only serves to look like another name.
 */
export function mixesScripts(label: string): boolean {
  return SCRIPTS.filter((script) => script.test(label)).length > 1;
}

/** Letters in a word before its capitals or digits are judged; shorter words are often initials or codes. */
export const DISGUISED_WORD_MIN_LETTERS = 4;

/** Characters that draw nothing, used to split a word so filters miss it. */
const INVISIBLE = "\u00AD\u200B-\u200D\u2060\uFEFF";
const INVISIBLE_ANY = new RegExp(`[${INVISIBLE}]`, "gu");
const INVISIBLE_INSIDE = new RegExp(`(\\p{L})[${INVISIBLE}]+(?=(\\p{L}))`, "gu");
const WORD = new RegExp(`[\\p{L}\\p{N}\\p{M}${INVISIBLE}]+`, "gu");
const LETTER = /\p{L}/u;
/** Digits drawn like letters, with the capital and small letter each stands for. */
const DIGIT_LETTERS: Record<string, [string, string]> = { "0": ["O", "o"], "1": ["I", "l"], "3": ["E", "e"], "5": ["S", "s"] };

/** A word written with look-alike characters, and how: "a lowercase l in place of I". */
export type DisguisedWord = { word: string; trick: string };

const isUpper = (character: string) => character !== character.toLocaleLowerCase();
const isLower = (character: string) => character !== character.toLocaleUpperCase();
const isLetter = (character: string | undefined) => character !== undefined && LETTER.test(character);
const scriptOf = (character: string) => SCRIPTS.findIndex((script) => script.test(character));

function scriptTrick(word: string): string | null {
  if (!mixesScripts(word)) return null;
  const letters = [...word].filter((character) => scriptOf(character) >= 0);
  const counts = SCRIPTS.map((_, index) => letters.filter((character) => scriptOf(character) === index).length);
  const main = counts.indexOf(Math.max(...counts));
  const foreign = letters.filter((character) => scriptOf(character) !== main);
  // Only letters drawn like the word's own alphabet disguise it; a symbol
  // such as Greek Δ beside Latin letters reads as what it is.
  const lookalike = foreign.filter((character) => CONFUSABLE[character.toLocaleLowerCase()] !== undefined && !/\d/.test(character));
  if (lookalike.length === 0) return null;
  const shown = SCRIPT_NAMES[scriptOf(lookalike[0])];
  return foreign.length === 1
    ? `a ${shown} letter in place of a ${SCRIPT_NAMES[main]} one`
    : `${shown} letters in place of ${SCRIPT_NAMES[main]} ones`;
}

function caseTrick(word: string): string | null {
  const characters = [...word];
  const letters = characters.filter(isLetter);
  if (letters.length < DISGUISED_WORD_MIN_LETTERS || letters.length !== characters.length) return null;
  const lower = letters.filter(isLower);
  const upper = letters.filter(isUpper);
  // A capitals word with a small l between capitals, or at its end: REVlEW, PAYPAl.
  if (lower.length > 0 && lower.every((character) => character === "l") && upper.length >= DISGUISED_WORD_MIN_LETTERS - 1) {
    const hidden = characters.some((character, index) => character === "l" && isUpper(characters[index - 1] ?? "")
      && (index === characters.length - 1 || isUpper(characters[index + 1])));
    if (hidden) return "a lowercase l in place of I";
  }
  // A small-letter word ending in a capital I: paypaI. Inside a word a capital
  // I usually starts the next part of a compound name, so only the end counts.
  if (upper.length === 1 && characters.at(-1) === "I" && lower.length >= DISGUISED_WORD_MIN_LETTERS - 1) {
    return "a capital I in place of l";
  }
  return null;
}

function digitTrick(word: string): string | null {
  const characters = [...word];
  const letters = characters.filter(isLetter);
  if (letters.length < DISGUISED_WORD_MIN_LETTERS) return null;
  const digits = characters.filter((character) => /\p{N}/u.test(character));
  // Codes and versions use any digit; a disguise only those drawn like letters.
  if (digits.length === 0 || digits.some((digit) => !DIGIT_LETTERS[digit])) return null;
  const capitals = letters.every(isUpper);
  // A disguised word keeps one style: ACC0UNT, Micr0soft, acc0unt.
  if (!capitals && !letters.slice(1).every(isLower)) return null;
  const index = characters.findIndex((character, at) => DIGIT_LETTERS[character]
    && isLetter(characters[at - 1]) && isLetter(characters[at - 2]) && isLetter(characters[at + 1]) && isLetter(characters[at + 2]));
  if (index < 0) return null;
  const digit = characters[index];
  return `the digit ${digit} in place of ${DIGIT_LETTERS[digit][capitals ? 0 : 1]}`;
}

/**
 * Words in `text`, such as a subject line, written with characters that
 * look like other letters so a reader sees a word a filter does not: an
 * invisible character inside a word, a letter from another alphabet, a
 * lowercase l among capitals, a capital I closing a small-letter word, or a
 * digit drawn like a letter between letters. Judged without a dictionary,
 * so it works the same in every language.
 */
export function disguisedWords(text: string): DisguisedWord[] {
  const found: DisguisedWord[] = [];
  for (const [raw] of text.matchAll(WORD)) {
    const word = raw.replace(INVISIBLE_ANY, "");
    // Joiners shape letters in scripts such as Arabic or Devanagari; between
    // the look-alike alphabets they only hide a word.
    const hidden = [...raw.matchAll(INVISIBLE_INSIDE)].some(([, before, after]) => scriptOf(before) >= 0 && scriptOf(after) >= 0);
    const trick = hidden ? "an invisible character between letters" : scriptTrick(word) ?? caseTrick(word) ?? digitTrick(word);
    if (trick) found.push({ word, trick });
  }
  return found;
}

/** Whether `left` becomes `right` by adding, removing, or swapping two adjacent characters once. */
function oneTypoApart(left: string, right: string): boolean {
  if (left === right) return false;
  if (left.length === right.length) {
    let first = 0;
    while (first < left.length && left[first] === right[first]) first += 1;
    return first + 1 < left.length && left[first] === right[first + 1] && left[first + 1] === right[first]
      && left.slice(first + 2) === right.slice(first + 2);
  }
  const [short, long] = left.length < right.length ? [left, right] : [right, left];
  if (long.length - short.length !== 1) return false;
  let index = 0;
  while (index < short.length && short[index] === long[index]) index += 1;
  return short.slice(index) === long.slice(index + 1);
}

/** A site split into its registrable name and the suffix after it: `example` and `co.uk`. */
function splitSite(site: string): { name: string; suffix: string } {
  const dot = site.indexOf(".");
  return dot < 0 ? { name: site, suffix: "" } : { name: site.slice(0, dot), suffix: site.slice(dot + 1) };
}

/** Known sites prepared once for comparing many candidates against. */
export type KnownSites = { sites: Set<string>; bySkeleton: Map<string, string>; byNameSkeleton: Map<string, string[]> };

export function knownSites(sites: Iterable<string>): KnownSites {
  const prepared: KnownSites = { sites: new Set(), bySkeleton: new Map(), byNameSkeleton: new Map() };
  for (const raw of sites) {
    const site = raw.toLocaleLowerCase();
    if (!site || prepared.sites.has(site)) continue;
    prepared.sites.add(site);
    const shown = unicodeHost(site);
    const folded = skeleton(shown);
    if (!prepared.bySkeleton.has(folded)) prepared.bySkeleton.set(folded, site);
    const key = skeleton(splitSite(shown).suffix);
    prepared.byNameSkeleton.set(key, [...(prepared.byNameSkeleton.get(key) ?? []), site]);
  }
  return prepared;
}

/**
 * The known site `site` imitates, or null: one that looks the same once
 * look-alike letters fold together, or one whose name has at least
 * LOOKALIKE_MIN_TYPO_LENGTH characters and is a single added, dropped, or
 * swapped letter away under the same suffix. A known site never imitates
 * another; two real correspondents can have similar names.
 */
export function lookalikeOf(site: string, known: KnownSites): string | null {
  const lower = site.toLocaleLowerCase();
  if (!lower || known.sites.has(lower)) return null;
  const shown = unicodeHost(lower);
  const folded = skeleton(shown);
  const same = known.bySkeleton.get(folded);
  if (same) return same;
  const { name, suffix } = splitSite(shown);
  const foldedName = skeleton(name);
  for (const candidate of known.byNameSkeleton.get(skeleton(suffix)) ?? []) {
    const candidateName = splitSite(unicodeHost(candidate)).name;
    if (candidateName.length < LOOKALIKE_MIN_TYPO_LENGTH) continue;
    // Folding can change a name's length (rn to m), so a typo is looked for both ways.
    if (oneTypoApart(name, candidateName) || oneTypoApart(foldedName, skeleton(candidateName))) return candidate;
  }
  return null;
}

/**
 * The known site a host spells out in front of its real one, such as
 * `example.com.login.example.net`, or null.
 */
export function spelledOutSite(host: string, site: string, known: KnownSites): string | null {
  const lower = unicodeHost(host).replace(/\.$/, "");
  const real = unicodeHost(site);
  if (!lower.endsWith(`.${real}`)) return null;
  const labels = lower.slice(0, -real.length - 1).split(".");
  for (let start = 0; start < labels.length - 1; start += 1) {
    for (let end = start + 2; end <= labels.length; end += 1) {
      const candidate = labels.slice(start, end).join(".");
      if (candidate !== real && known.sites.has(candidate)) return candidate;
    }
  }
  return null;
}
