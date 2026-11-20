import { useCallback, useEffect, useState } from "react";
import { CalendarDays, CheckCircle, Notebook, Plus, Settings } from "lucide-react";
import { noteTargetKey, parseNoteTargetKey, todayTarget, type NoteTarget } from "../../src/noteDrafts";
import type { NoteView } from "./api";
import { NotesList } from "./NotesList";
import { DailyNavigation, NewNote, NoteScreen } from "./NoteScreen";
import { SyncScreen } from "./SyncScreen";
import { EditTask, TaskForm, TasksScreen } from "./TasksScreen";
import { useSync } from "./useSync";

type Page =
  | { screen: "daily"; date: string; edit?: boolean }
  | { screen: "note"; target: NoteTarget; marker?: string }
  | { screen: "notes" | "tasks" | "sync" | "newNote" | "newTask" }
  | { screen: "task"; id: string };

const dailyPage = (): Page => ({ screen: "daily", date: todayTarget().date });

export function App() {
  const [today, setToday] = useState(() => todayTarget().date);
  const [page, setPage] = useState<Page>(dailyPage);
  const [revision, setRevision] = useState(0);
  const [query, setQuery] = useState("");
  const [noteView, setNoteView] = useState<NoteView>("recent");
  const [showDone, setShowDone] = useState(false);
  const [capturing, setCapturing] = useState(false);
  const changed = useCallback(() => setRevision(value => value + 1), []);
  const sync = useSync(changed);

  useEffect(() => {
    window.history.replaceState(dailyPage(), "");
    const goBack = (event: PopStateEvent) => {
      setPage(event.state ?? dailyPage());
      setCapturing(false);
    };
    const updateToday = () => setToday(todayTarget().date);
    const interval = window.setInterval(updateToday, 60_000);
    window.addEventListener("popstate", goBack);
    document.addEventListener("visibilitychange", updateToday);
    return () => {
      window.clearInterval(interval);
      window.removeEventListener("popstate", goBack);
      document.removeEventListener("visibilitychange", updateToday);
    };
  }, []);

  function navigate(next: Page, replace = false) {
    setCapturing(false);
    if (replace) window.history.replaceState(next, "");
    else window.history.pushState(next, "");
    setPage(next);
    window.scrollTo(0, 0);
  }

  function openNote(target: NoteTarget) {
    navigate(target.kind === "daily" ? { screen: "daily", date: target.date } : { screen: "note", target });
  }

  const tabs = [
    { label: "Daily", icon: CalendarDays, active: page.screen === "daily", next: dailyPage() },
    { label: "Notes", icon: Notebook, active: ["notes", "note", "newNote"].includes(page.screen), next: { screen: "notes" } as Page },
    { label: "Tasks", icon: CheckCircle, active: ["tasks", "task", "newTask"].includes(page.screen), next: { screen: "tasks" } as Page },
  ];

  return <>
    <header className="app-bar">
      <span className="wordmark">ThreeStrands</span>
      <button className="icon-button" aria-label="Sync settings" aria-current={page.screen === "sync" ? "page" : undefined}
        onClick={() => navigate({ screen: "sync" })}><Settings size={22} /></button>
    </header>
    <main>
      {page.screen === "daily" && <>
        <DailyNavigation date={page.date} today={today} onDate={date => navigate({ screen: "daily", date }, true)} />
        <div className="capture-row">
          <button className="primary" aria-expanded={capturing} onClick={() => setCapturing(value => !value)}><Plus size={19} /> Capture</button>
          {capturing && <div className="capture-actions" aria-label="Capture options">
            <button onClick={() => navigate({ ...page, edit: true }, true)}>Add to Daily Note</button>
            <button onClick={() => navigate({ screen: "newNote" })}>New Note</button>
            <button onClick={() => navigate({ screen: "newTask" })}>New Task</button>
          </div>}
        </div>
        <NoteScreen key={`${page.date}:${page.edit ?? false}`} target={{ kind: "daily", date: page.date }}
          revision={revision} onOpen={openNote} onChange={changed} startEditing={page.edit} />
      </>}
      {page.screen === "notes" && <NotesList revision={revision} onOpen={openNote} onNew={() => navigate({ screen: "newNote" })}
        query={query} setQuery={setQuery} view={noteView} setView={setNoteView} />}
      {page.screen === "note" && <NoteScreen key={noteTargetKey(page.target)} target={page.target} revision={revision}
        onOpen={openNote} onChange={changed} focused={page.marker} />}
      {page.screen === "newNote" && <NewNote onSave={note => {
        changed();
        navigate({ screen: "note", target: parseNoteTargetKey(note.id) }, true);
      }} onCancel={() => window.history.back()} />}
      {page.screen === "tasks" && <TasksScreen revision={revision} onChange={changed}
        done={showDone} setDone={setShowDone} onNew={() => navigate({ screen: "newTask" })}
        onEdit={id => navigate({ screen: "task", id })}
        onOpenNote={item => navigate({ screen: "note", target: parseNoteTargetKey(item.noteId), marker: item.markerId })} />}
      {page.screen === "newTask" && <TaskForm onSaved={() => {
        changed();
        setShowDone(false);
        navigate({ screen: "tasks" }, true);
      }} onCancel={() => window.history.back()} />}
      {page.screen === "task" && <EditTask id={page.id} onSaved={() => { changed(); window.history.back(); }}
        onCancel={() => window.history.back()} />}
      {page.screen === "sync" && <SyncScreen state={sync} />}
    </main>
    <nav aria-label="Workspaces" className="tab-bar">
      {tabs.map(({ label, icon: Icon, active, next }) => <button key={label} aria-current={active ? "page" : undefined}
        onClick={() => { if (page.screen !== next.screen) navigate(next); }}>
        <Icon size={21} aria-hidden="true" /><span>{label}</span>
      </button>)}
    </nav>
  </>;
}
