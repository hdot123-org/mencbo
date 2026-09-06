import type { TaskState } from "../types";
import { TaskCard } from "./TaskCard";

interface TaskListProps {
  tasks: TaskState[];
}

export function TaskList({ tasks }: TaskListProps) {
  if (tasks.length === 0) {
    return (
      <div
        data-testid="empty-state"
        className="flex items-center justify-center h-full text-neutral-500 text-sm"
      >
        暂无任务
      </div>
    );
  }

  return (
    <div data-testid="task-list" className="divide-y divide-neutral-800">
      {tasks.map((task) => (
        <TaskCard key={task.id} task={task} />
      ))}
    </div>
  );
}
