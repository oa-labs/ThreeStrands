/** `count` followed by the singular or plural form of `noun`, e.g. "1 change", "3 changes". */
export function plural(count: number, noun: string, pluralNoun = `${noun}s`): string {
  return `${count} ${count === 1 ? noun : pluralNoun}`;
}
