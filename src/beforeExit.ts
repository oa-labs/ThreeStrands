/**
 * Work that must finish before the app closes, such as saving a note. The
 * window's close is held while the composer saves its draft
 * (`useCorrespondence`), which then runs these before letting the app exit.
 */
const tasks = new Set<() => Promise<void>>();

export function onBeforeExit(task: () => Promise<void>): () => void {
  tasks.add(task);
  return () => { tasks.delete(task); };
}

/** Runs every registered task. One failing doesn't stop the others or the exit. */
export async function runBeforeExitTasks(): Promise<void> {
  await Promise.allSettled([...tasks].map((task) => task()));
}
