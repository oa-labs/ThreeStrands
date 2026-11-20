import { useCallback, useEffect, useState } from "react";
import { CalendarDays, CheckCircle, ChevronLeft, Notebook, Plus, Settings } from "lucide-react";
import { ACCENTS, readAccent, saveAccent, type Accent } from "../../src/accent";
import { TODAY_BACKDROP_KEY, writeTodayBackdrop } from "../../src/backdropPreference";
import { noteTargetKey, parseNoteTargetKey, todayTarget, type NoteTarget } from "../../src/noteDrafts";
import { readLocalStorage } from "../../src/useLocalStorageState";
import type { NoteView } from "./api";
import { NotesList } from "./NotesList";
import { DailyNavigation, NewNote, NoteScreen, formatDailyHeading } from "./NoteScreen";
import { SyncScreen } from "./SyncScreen";
import { EditTask, TaskForm, TasksScreen } from "./TasksScreen";
import { useMobileNavigation } from "./useMobileNavigation";
import { useSync } from "./useSync";

const tabs = [
  { workspace: "daily", label: "Daily", icon: CalendarDays },
  { workspace: "notes", label: "Notes", icon: Notebook },
  { workspace: "tasks", label: "Tasks", icon: CheckCircle },
] as const;

export function App() {
  const [now, setNow] = useState(() => new Date());
  const today = todayTarget(now).date;
  const { page, workspace, entryId, scroller, content, rememberScroll, navigate, switchWorkspace, back } = useMobileNavigation();
  const [revision, setRevision] = useState(0);
  const [query, setQuery] = useState("");
  const [noteView, setNoteView] = useState<Exclude<NoteView, "trash">>("recent");
  const [showDone, setShowDone] = useState(false);
  const [captureEntry, setCaptureEntry] = useState<number>();
  const [noteHeading, setNoteHeading] = useState<Record<number, string>>({});
  const [accent, setAccent] = useState(readAccent);
  const [backdrop, setBackdrop] = useState(() => readLocalStorage(TODAY_BACKDROP_KEY, raw => raw !== "off", true));
  const updateNoteHeading = useCallback((title: string) => {
    setNoteHeading(previous => previous[entryId] === title ? previous : { ...previous, [entryId]: title });
  }, [entryId]);
  const changed = useCallback(() => setRevision(value => value + 1), []);
  const sync = useSync(changed);

  useEffect(() => {
    const updateNow = () => setNow(new Date());
    const interval = window.setInterval(updateNow, 60_000);
    document.addEventListener("visibilitychange", updateNow);
    window.addEventListener("threestrands-resume", updateNow);
    return () => {
      window.clearInterval(interval);
      document.removeEventListener("visibilitychange", updateNow);
      window.removeEventListener("threestrands-resume", updateNow);
    };
  }, []);

  function openNote(target: NoteTarget) {
    navigate(target.kind === "daily" ? { screen: "daily", date: target.date } : { screen: "note", target });
  }

  const screen = page.screen;
  const topLevel = screen === "daily" || screen === "notes" || screen === "tasks";
  const heading = screen === "daily" ? formatDailyHeading(page.date, now)
    : screen === "note" ? noteHeading[entryId] ?? "Note"
      : { notes: "Notes", tasks: "Tasks", trash: "Trash", settings: "Settings", newNote: "New Note", newTask: "New Task", task: "Task details" }[screen];
  const creation = screen === "notes" ? "New Note" : screen === "tasks" ? "New Task" : "Capture";
  const capturing = captureEntry === entryId;

  return <>
    <header className="app-bar">
      <div className="app-bar-inner">
        {!topLevel && <button className="icon-button" aria-label="Back" onClick={back}><ChevronLeft aria-hidden="true" size={22} /></button>}
        <h1 title={screen === "note" ? heading : undefined}>{heading}</h1>
        {topLevel && <>
          <button className="primary create-button" aria-expanded={screen === "daily" ? capturing : undefined}
            aria-controls={screen === "daily" && capturing ? "capture-options" : undefined}
            onClick={() => {
              if (screen === "daily") setCaptureEntry(capturing ? undefined : entryId);
              else navigate({ screen: screen === "notes" ? "newNote" : "newTask" });
            }}><Plus size={18} aria-hidden="true" />{creation}</button>
          <button className="icon-button" aria-label="Settings" onClick={() => { setCaptureEntry(undefined); navigate({ screen: "settings" }); }}><Settings size={21} aria-hidden="true" /></button>
        </>}
      </div>
    </header>
    <main ref={scroller} onScroll={rememberScroll}>
      <div className="main-content" ref={content}>
        {page.screen === "daily" && <>
          <DailyNavigation date={page.date} today={today} now={now} backdrop={backdrop}
            onDate={date => { if (date !== page.date) navigate({ screen: "daily", date }, { replace: true }); }} />
          {capturing && <div id="capture-options" className="capture-actions" aria-label="Capture options">
            <button onClick={() => navigate({ ...page, edit: true }, { replace: true })}>Add to Daily Note</button>
            <button onClick={() => { setCaptureEntry(undefined); navigate({ screen: "newNote" }); }}>New Note</button>
            <button onClick={() => { setCaptureEntry(undefined); navigate({ screen: "newTask" }); }}>New Task</button>
          </div>}
          <NoteScreen key={entryId} target={{ kind: "daily", date: page.date }}
            revision={revision} onOpen={openNote} onChange={changed} startEditing={page.edit} />
        </>}
        {(page.screen === "notes" || page.screen === "trash") && <NotesList revision={revision} onOpen={openNote}
          query={query} setQuery={setQuery} view={page.screen === "trash" ? "trash" : noteView} setView={setNoteView}
          onTrash={() => navigate({ screen: "trash" })} />}
        {page.screen === "note" && <NoteScreen key={noteTargetKey(page.target)} target={page.target} revision={revision}
          onOpen={openNote} onChange={changed} focused={page.marker}
          onTitle={updateNoteHeading} />}
        {page.screen === "newNote" && <NewNote onSave={note => {
          changed();
          navigate({ screen: "note", target: parseNoteTargetKey(note.id) }, { replace: true });
        }} onCancel={back} />}
        {page.screen === "tasks" && <TasksScreen revision={revision} onChange={changed}
          done={showDone} setDone={setShowDone} onEdit={id => navigate({ screen: "task", id })}
          onOpenNote={item => navigate({ screen: "note", target: parseNoteTargetKey(item.noteId), marker: item.markerId })} />}
        {page.screen === "newTask" && <TaskForm onSaved={() => {
          changed();
          setShowDone(false);
          navigate({ screen: "tasks" }, { replace: true });
        }} onCancel={back} />}
        {page.screen === "task" && <EditTask id={page.id} onSaved={() => { changed(); back(); }} onCancel={back} />}
        {page.screen === "settings" && <>
          <section className="appearance" aria-labelledby="appearance-heading">
            <h2 id="appearance-heading">Appearance</h2>
            <label>Accent color<select value={accent} onChange={event => {
              const value = event.target.value as Accent;
              setAccent(value);
              saveAccent(value);
            }}>{ACCENTS.map(value => <option key={value} value={value}>{value[0].toUpperCase() + value.slice(1)}</option>)}</select></label>
            <label className="appearance-toggle"><input type="checkbox" checked={backdrop} onChange={event => {
              setBackdrop(event.target.checked);
              writeTodayBackdrop(event.target.checked);
            }} />Daily backdrop</label>
          </section>
          <SyncScreen state={sync} />
          <p className="wordmark">ThreeStrands</p>
        </>}
      </div>
    </main>
    <nav aria-label="Workspaces" className="tab-bar">
      {tabs.map(({ workspace: target, label, icon: Icon }) => <button key={target} aria-current={workspace === target ? "page" : undefined}
        onClick={() => { setCaptureEntry(undefined); switchWorkspace(target); }}>
        <span className="tab-icon"><Icon size={20} aria-hidden="true" /></span><span>{label}</span>
      </button>)}
    </nav>
  </>;
}
