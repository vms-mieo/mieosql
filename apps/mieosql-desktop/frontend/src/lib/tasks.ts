import { create } from "zustand";

export type TaskStatus = "running" | "done" | "error";
export type TaskKind = "export" | "backup" | "sync" | "transfer" | "other";

export interface Task {
  id: string;
  title: string;
  kind: TaskKind;
  status: TaskStatus;
  startedAt: number;
  elapsedMs?: number;
  detail?: string;
  error?: string;
}

interface TasksState {
  tasks: Task[];
  pushTask: (title: string, kind?: TaskKind) => string;
  updateTask: (id: string, patch: Partial<Task>) => void;
  clearFinished: () => void;
}

let seq = 0;

export const useTasks = create<TasksState>((set) => ({
  tasks: [],
  pushTask: (title, kind = "other") => {
    const id = `task-${Date.now()}-${(seq += 1)}`;
    const task: Task = {
      id,
      title,
      kind,
      status: "running",
      startedAt: Date.now(),
    };
    set((s) => ({ tasks: [task, ...s.tasks].slice(0, 50) }));
    return id;
  },
  updateTask: (id, patch) =>
    set((s) => ({
      tasks: s.tasks.map((t) => (t.id === id ? { ...t, ...patch } : t)),
    })),
  clearFinished: () =>
    set((s) => ({ tasks: s.tasks.filter((t) => t.status === "running") })),
}));

/** Run an async job tracked in the Background Tasks drawer. No toasts here. */
export async function runWithTask<T>(
  title: string,
  fn: (report: (detail: string) => void) => Promise<T>,
  kind: TaskKind = "other",
): Promise<T> {
  const { pushTask, updateTask } = useTasks.getState();
  const id = pushTask(title, kind);
  const started = Date.now();
  const report = (detail: string) => updateTask(id, { detail });
  try {
    const result = await fn(report);
    updateTask(id, { status: "done", elapsedMs: Date.now() - started });
    return result;
  } catch (err) {
    updateTask(id, {
      status: "error",
      elapsedMs: Date.now() - started,
      error: err instanceof Error ? err.message : String(err),
    });
    throw err;
  }
}

/** Ask the Tasks drawer to open (dispatched by StatusBar, heard by TasksDrawer). */
export function toggleTasksDrawer() {
  window.dispatchEvent(new CustomEvent("mieosql:toggle-tasks"));
}
