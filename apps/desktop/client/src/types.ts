export interface TaskState {
  id: string;
  name: string;
  status: "success" | "running" | "failed";
  lastRun: string | null;
  durationMs: number | null;
  error?: string | null;
}

export interface State {
  mock?: boolean;
  health?: "ok" | "degraded";
  tasks: TaskState[];
}
