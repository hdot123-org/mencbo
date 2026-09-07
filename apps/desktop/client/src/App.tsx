import { useEffect, useState } from "react";
import { HeaderBar } from "./components/HeaderBar";
import { TaskList } from "./components/TaskList";
import { FooterBar } from "./components/FooterBar";
import { loadState, subscribeState } from "./lib/state";
import { capture } from "./lib/analytics";
import type { State } from "./types";

function App() {
  const [state, setState] = useState<State | null>(null);

  useEffect(() => {
    const t0 = performance.now();
    loadState()
      .then((s) => {
        setState(s);
        capture("state_load", {
          tasks: Array.isArray(s.tasks) ? s.tasks.length : 0,
          duration_ms: Math.round(performance.now() - t0),
        });
      })
      .catch((e) => capture("diag_state_load_failed", { reason: String(e).slice(0, 200) }));
    const unsubscribe = subscribeState((s) => {
      setState(s);
      capture("state_sync", {
        tasks: Array.isArray(s.tasks) ? s.tasks.length : 0,
      });
    });
    return unsubscribe;
  }, []);

  if (!state) {
    return <div className="h-screen w-screen bg-neutral-900" />;
  }

  // Defensive default: ensure tasks is always an array even if state shape is malformed
  const tasks = Array.isArray(state.tasks) ? state.tasks : [];
  const health = state.health ?? (tasks.some((t) => t.status === "failed") ? "degraded" : "ok");

  return (
    <div className="flex flex-col h-screen w-screen bg-neutral-900 text-neutral-100">
      {state.mock && (
        <div
          data-testid="mock-badge"
          className="absolute top-2 right-2 px-2 py-0.5 text-[10px] font-medium text-neutral-400 bg-neutral-800 rounded z-10"
        >
          MOCK
        </div>
      )}
      <HeaderBar health={health as "ok" | "degraded"} />
      <div className="flex-1 overflow-y-auto">
        <TaskList tasks={tasks} />
      </div>
      <FooterBar />
    </div>
  );
}

export default App;
