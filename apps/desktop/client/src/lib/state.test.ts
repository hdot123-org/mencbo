import { describe, it, expect } from 'vitest';
import { MOCK_STATE } from '../test/fixtures';
import type { State } from '../types';

describe('VAL-STATE-007: browser mock data layer', () => {
  describe('MOCK_STATE contains all demo statuses', () => {
    it('should have exactly 3 tasks', () => {
      expect(MOCK_STATE.tasks).toHaveLength(3);
    });

    it('should contain at least one success task', () => {
      const successTasks = MOCK_STATE.tasks.filter(t => t.status === 'success');
      expect(successTasks.length).toBeGreaterThan(0);
    });

    it('should contain at least one running task', () => {
      const runningTasks = MOCK_STATE.tasks.filter(t => t.status === 'running');
      expect(runningTasks.length).toBeGreaterThan(0);
    });

    it('should contain at least one failed task', () => {
      const failedTasks = MOCK_STATE.tasks.filter(t => t.status === 'failed');
      expect(failedTasks.length).toBeGreaterThan(0);
    });
  });

  describe('triggerTask mock behavior', () => {
    it('should resolve without error in browser mode', async () => {
      // Dynamically import to test browser fallback
      const { triggerTask } = await import('./state');

      // In browser mode (non-Tauri), triggerTask should resolve without throwing
      await expect(triggerTask('example:heartbeat')).resolves.not.toThrow();
    });

    it('should handle various task IDs without error', async () => {
      const { triggerTask } = await import('./state');

      await expect(triggerTask('example:heartbeat')).resolves.toBeUndefined();
      await expect(triggerTask('example:maintenance')).resolves.toBeUndefined();
      await expect(triggerTask('example:failure-demo')).resolves.toBeUndefined();
      await expect(triggerTask('nonexistent:task')).resolves.toBeUndefined();
    });
  });

  describe('loadState mock behavior', () => {
    it('should return MOCK_STATE in browser mode', async () => {
      const { loadState } = await import('./state');

      const state = await loadState();

      // Should return mock state with all required fields
      expect(state).toBeDefined();
      expect(state.mock).toBe(true);
      expect(state.tasks).toBeInstanceOf(Array);
      expect(state.tasks.length).toBe(3);
    });

    it('should return state with correct task structure', async () => {
      const { loadState } = await import('./state');

      const state = await loadState();

      state.tasks.forEach(task => {
        expect(task).toHaveProperty('id');
        expect(task).toHaveProperty('name');
        expect(task).toHaveProperty('status');
        expect(['success', 'running', 'failed']).toContain(task.status);
      });
    });
  });

  describe('subscribeState mock behavior', () => {
    it('should return unsubscribe function', async () => {
      const { subscribeState } = await import('./state');

      const callback = (_state: State) => {};
      const unsubscribe = subscribeState(callback);

      expect(typeof unsubscribe).toBe('function');

      // Should not throw when called
      expect(() => unsubscribe()).not.toThrow();
    });

    it('should accept callback without error', async () => {
      const { subscribeState } = await import('./state');

      const callback = (state: State) => {
        expect(state).toBeDefined();
      };

      expect(() => subscribeState(callback)).not.toThrow();
    });
  });
});
