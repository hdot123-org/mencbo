import { Play } from "lucide-react";
import type { TaskState } from "../types";
import { formatDuration, formatRelativeTime } from "../lib/format";
import { triggerTask } from "../lib/state";

interface TaskCardProps {
  task: TaskState;
}

export function TaskCard({ task }: TaskCardProps) {
  const statusColor = {
    success: "bg-emerald-400",
    running: "bg-blue-400 animate-pulse",
    failed: "bg-red-400",
  }[task.status];

  const statusLabel = {
    success: "成功",
    running: "运行中",
    failed: "失败",
  }[task.status];

  return (
    <div
      data-testid="task-card"
      data-task-id={task.id}
      className="px-4 py-3 border-b border-neutral-800 hover:bg-neutral-800/50 transition-colors"
    >
      <div className="flex items-start justify-between gap-3">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 mb-1">
            <span data-testid="task-name" className="text-sm font-medium text-neutral-100">
              {task.name}
            </span>
          </div>
          <div className="flex items-center gap-3 text-xs text-neutral-400">
            <span data-testid="task-last-run">
              {formatRelativeTime(task.lastRun)}
            </span>
            <span data-testid="task-duration">
              {formatDuration(task.durationMs)}
            </span>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <div
            data-testid="task-status"
            data-status={task.status}
            className={`w-2 h-2 rounded-full ${statusColor}`}
            title={statusLabel}
          />
          <button
            data-testid="trigger-btn"
            data-task-id={task.id}
            disabled={task.status === "running"}
            onClick={() => triggerTask(task.id)}
            className="flex items-center gap-1 px-2 py-1 text-xs font-medium text-neutral-200 bg-neutral-700 hover:bg-neutral-600 disabled:opacity-40 disabled:cursor-not-allowed rounded transition-colors"
          >
            <Play size={12} />
            立即触发
          </button>
        </div>
      </div>
    </div>
  );
}
