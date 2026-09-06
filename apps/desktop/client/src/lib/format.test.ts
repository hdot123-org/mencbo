import { describe, it, expect } from 'vitest';
import { formatRelativeTime, formatDuration } from './format';

describe('formatRelativeTime', () => {
  const baseTime = new Date('2024-01-01T12:00:00Z');

  describe('VAL-PANEL-012: relative time pure function boundaries', () => {
    it('should return "刚刚" for offset 0s', () => {
      const target = new Date(baseTime.getTime() - 0 * 1000);
      expect(formatRelativeTime(target.toISOString(), baseTime)).toBe('刚刚');
    });

    it('should return "刚刚" for offset 30s', () => {
      const target = new Date(baseTime.getTime() - 30 * 1000);
      expect(formatRelativeTime(target.toISOString(), baseTime)).toBe('刚刚');
    });

    it('should return "刚刚" for offset 59s', () => {
      const target = new Date(baseTime.getTime() - 59 * 1000);
      expect(formatRelativeTime(target.toISOString(), baseTime)).toBe('刚刚');
    });

    it('should return "1 分钟前" for offset 60s', () => {
      const target = new Date(baseTime.getTime() - 60 * 1000);
      expect(formatRelativeTime(target.toISOString(), baseTime)).toBe('1 分钟前');
    });

    it('should return "59 分钟前" for offset 3599s', () => {
      const target = new Date(baseTime.getTime() - 3599 * 1000);
      expect(formatRelativeTime(target.toISOString(), baseTime)).toBe('59 分钟前');
    });

    it('should return "1 小时前" for offset 3600s', () => {
      const target = new Date(baseTime.getTime() - 3600 * 1000);
      expect(formatRelativeTime(target.toISOString(), baseTime)).toBe('1 小时前');
    });

    it('should return "23 小时前" for offset 86399s', () => {
      const target = new Date(baseTime.getTime() - 86399 * 1000);
      expect(formatRelativeTime(target.toISOString(), baseTime)).toBe('23 小时前');
    });

    it('should return "1 天前" for offset 86400s', () => {
      const target = new Date(baseTime.getTime() - 86400 * 1000);
      expect(formatRelativeTime(target.toISOString(), baseTime)).toBe('1 天前');
    });

    it('should return "3 天前" for offset 259200s', () => {
      const target = new Date(baseTime.getTime() - 259200 * 1000);
      expect(formatRelativeTime(target.toISOString(), baseTime)).toBe('3 天前');
    });
  });
});

describe('formatDuration', () => {
  describe('VAL-PANEL-013: duration formatting boundaries', () => {
    it('should return "0ms" for 0', () => {
      expect(formatDuration(0)).toBe('0ms');
    });

    it('should return "12ms" for 12', () => {
      expect(formatDuration(12)).toBe('12ms');
    });

    it('should return "320ms" for 320', () => {
      expect(formatDuration(320)).toBe('320ms');
    });

    it('should return "999ms" for 999', () => {
      expect(formatDuration(999)).toBe('999ms');
    });

    it('should return "1.0s" for 1000', () => {
      expect(formatDuration(1000)).toBe('1.0s');
    });

    it('should return "1.2s" for 1234', () => {
      expect(formatDuration(1234)).toBe('1.2s');
    });

    it('should return "59.4s" for 59400', () => {
      expect(formatDuration(59400)).toBe('59.4s');
    });

    it('should return "120.0s" for 120000', () => {
      expect(formatDuration(120000)).toBe('120.0s');
    });
  });

  describe('VAL-PANEL-013: null handling', () => {
    it('should handle null without throwing', () => {
      expect(() => formatDuration(null)).not.toThrow();
    });

    it('should return "—" for null', () => {
      expect(formatDuration(null)).toBe('—');
    });

    it('should handle undefined without throwing', () => {
      expect(() => formatDuration(undefined)).not.toThrow();
    });

    it('should return "—" for undefined', () => {
      expect(formatDuration(undefined)).toBe('—');
    });
  });
});
