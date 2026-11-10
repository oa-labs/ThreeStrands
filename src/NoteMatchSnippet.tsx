import { formatNoteDate } from "./noteDates";

/** FTS5 `snippet()` marks each match between these characters. */
const MATCH_START = "\u0001";
const MATCH_END = "\u0002";

/** A search hit's excerpt, with the matched words highlighted. */
export function NoteMatchSnippet({ text }: { text: string }) {
  const [before, ...matches] = text.split(MATCH_START);
  return <>{before}{matches.map((segment, index) => {
    const [match, ...rest] = segment.split(MATCH_END);
    return <span key={index}><mark>{match}</mark>{rest.join(MATCH_END)}</span>;
  })}</>;
}

/** A note's name as lists show it: a daily note's date, or the title. */
export const noteLabel = (note: { title: string; dailyDate: string | null }) => note.dailyDate ? formatNoteDate(note.dailyDate) : note.title;
