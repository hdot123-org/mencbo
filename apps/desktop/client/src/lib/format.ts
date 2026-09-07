/**
 * Format a duration in milliseconds to human-readable string.
 * < 1000ms → "12ms", >= 1000ms → "1.2s" (one decimal, never convert to minutes).
 * null/undefined → "—"
 */
export function formatDuration(ms: number | null | undefined): string {
  if (ms == null) return "—";
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

/**
 * Format an ISO timestamp to relative Chinese time string.
 * Buckets: 刚刚 / N 分钟前 / N 小时前 / N 天前
 */
export function formatRelativeTime(iso: string | null | undefined, now?: Date): string {
  if (!iso) return "—";
  const then = new Date(iso);
  if (isNaN(then.getTime())) return "—";
  const reference = now ?? new Date();
  const diffSec = Math.max(0, Math.floor((reference.getTime() - then.getTime()) / 1000));

  if (diffSec < 60) return "刚刚";
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)} 分钟前`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)} 小时前`;
  return `${Math.floor(diffSec / 86400)} 天前`;
}
