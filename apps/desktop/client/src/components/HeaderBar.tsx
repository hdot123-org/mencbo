import { Activity } from "lucide-react";

interface HeaderBarProps {
  health: "ok" | "degraded";
}

export function HeaderBar({ health }: HeaderBarProps) {
  return (
    <div className="flex items-center justify-between px-4 py-3 border-b border-neutral-800">
      <div className="flex items-center gap-2">
        <Activity size={16} className="text-neutral-400" />
        <span className="text-sm font-medium text-neutral-200">
          hdot123-org/mencbo
        </span>
      </div>
      <div
        data-testid="health-light"
        data-health={health}
        className={`w-2 h-2 rounded-full ${
          health === "ok"
            ? "bg-emerald-400 animate-pulse"
            : "bg-amber-400 animate-pulse"
        }`}
      />
    </div>
  );
}
