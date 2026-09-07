import { describe, expect, it } from "vitest";
import { formatDuration, formatRelativeTime } from "./format";

describe("formatDuration", () => {
  it("formats 0 as 0ms", () => {
    expect(formatDuration(0)).toBe("0ms");
  });

  it("formats 12 as 12ms", () => {
    expect(formatDuration(12)).toBe("12ms");
  });

  it("formats 320 as 320ms", () => {
    expect(formatDuration(320)).toBe("320ms");
  });

  it("formats 999 as 999ms", () => {
    expect(formatDuration(999)).toBe("999ms");
  });

  it("formats 1000 as 1.0s (one decimal, not 1s)", () => {
    expect(formatDuration(1000)).toBe("1.0s");
  });

  it("formats 1234 as 1.2s", () => {
    expect(formatDuration(1234)).toBe("1.2s");
  });

  it("formats 59400 as 59.4s", () => {
    expect(formatDuration(59400)).toBe("59.4s");
  });

  it("formats 120000 as 120.0s (no minute conversion)", () => {
    expect(formatDuration(120000)).toBe("120.0s");
  });

  it("formats null as placeholder", () => {
    const result = formatDuration(null);
    expect(result).toBe("—");
    expect(result).not.toContain("null");
    expect(result).not.toContain("NaN");
  });

  it("formats undefined as placeholder", () => {
    const result = formatDuration(undefined);
    expect(result).toBe("—");
    expect(result).not.toContain("null");
    expect(result).not.toContain("NaN");
  });
});

describe("formatRelativeTime", () => {
  const base = new Date("2026-09-07T12:00:00+00:00");

  it("formats 0s as 刚刚", () => {
    const target = new Date(base.getTime() - 0 * 1000);
    expect(formatRelativeTime(target.toISOString(), base)).toBe("刚刚");
  });

  it("formats 30s as 刚刚", () => {
    const target = new Date(base.getTime() - 30 * 1000);
    expect(formatRelativeTime(target.toISOString(), base)).toBe("刚刚");
  });

  it("formats 59s as 刚刚", () => {
    const target = new Date(base.getTime() - 59 * 1000);
    expect(formatRelativeTime(target.toISOString(), base)).toBe("刚刚");
  });

  it("formats 60s as 1 分钟前", () => {
    const target = new Date(base.getTime() - 60 * 1000);
    expect(formatRelativeTime(target.toISOString(), base)).toBe("1 分钟前");
  });

  it("formats 3599s as 59 分钟前", () => {
    const target = new Date(base.getTime() - 3599 * 1000);
    expect(formatRelativeTime(target.toISOString(), base)).toBe("59 分钟前");
  });

  it("formats 3600s as 1 小时前", () => {
    const target = new Date(base.getTime() - 3600 * 1000);
    expect(formatRelativeTime(target.toISOString(), base)).toBe("1 小时前");
  });

  it("formats 86399s as 23 小时前", () => {
    const target = new Date(base.getTime() - 86399 * 1000);
    expect(formatRelativeTime(target.toISOString(), base)).toBe("23 小时前");
  });

  it("formats 86400s as 1 天前", () => {
    const target = new Date(base.getTime() - 86400 * 1000);
    expect(formatRelativeTime(target.toISOString(), base)).toBe("1 天前");
  });

  it("formats 259200s (3 days) as 3 天前", () => {
    const target = new Date(base.getTime() - 259200 * 1000);
    expect(formatRelativeTime(target.toISOString(), base)).toBe("3 天前");
  });

  it("formats null as placeholder", () => {
    const result = formatRelativeTime(null, base);
    expect(result).toBe("—");
  });

  it("handles invalid ISO gracefully", () => {
    const result = formatRelativeTime("not-a-date", base);
    expect(result).toBe("—");
  });
});
