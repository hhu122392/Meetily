import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import { MOSS_COMMANDS, MossReviewService } from '../../src/features/moss/service';
import type { MossRunState, MossWorkspace } from '../../src/features/moss/types';
import { useMossWorkspace } from '../../src/features/moss/useMossWorkspace';
import { mossWorkspaceFixture } from './fixtures';

type TimerTask = {
  callback: TimerHandler;
  delay: number;
  repeating: boolean;
};

class FakeWindowTimers {
  private nextId = 1;
  readonly tasks = new Map<number, TimerTask>();

  readonly window = {
    setTimeout: (callback: TimerHandler, delay = 0) => this.add(callback, delay, false),
    clearTimeout: (id: number) => { this.tasks.delete(id); },
    setInterval: (callback: TimerHandler, delay = 0) => this.add(callback, delay, true),
    clearInterval: (id: number) => { this.tasks.delete(id); },
  };

  private add(callback: TimerHandler, delay: number, repeating: boolean): number {
    const id = this.nextId++;
    this.tasks.set(id, { callback, delay, repeating });
    return id;
  }

  delays(): number[] {
    return [...this.tasks.values()].map((task) => task.delay).sort((left, right) => left - right);
  }

  fireNext(): void {
    const entry = [...this.tasks.entries()].sort(([left], [right]) => left - right)[0];
    if (!entry) throw new Error('No pending fake timer');
    const [id, task] = entry;
    if (!task.repeating) this.tasks.delete(id);
    if (typeof task.callback !== 'function') throw new Error('String timer callbacks are not supported');
    task.callback();
  }
}

function installFakeWindow(): { timers: FakeWindowTimers; restore: () => void } {
  const originalWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
  const timers = new FakeWindowTimers();
  Object.defineProperty(globalThis, 'window', { configurable: true, value: timers.window });
  return {
    timers,
    restore: () => {
      if (originalWindow) Object.defineProperty(globalThis, 'window', originalWindow);
      else delete (globalThis as { window?: unknown }).window;
    },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((accept) => { resolve = accept; });
  return { promise, resolve };
}

async function flushMicrotasks(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

function workspaceWithState(state: MossRunState): MossWorkspace {
  const workspace = structuredClone(mossWorkspaceFixture());
  workspace.runs[0].state = state;
  workspace.runs[0].canCancel = state === 'running';
  return workspace;
}

test('cancel_requested starts a new polling cycle with an immediate refresh', async () => {
  const fakeWindow = installFakeWindow();
  let serverState: MossRunState = 'running';
  let getWorkspaceCalls = 0;
  const service = new MossReviewService(async <T,>(command: string) => {
    if (command === MOSS_COMMANDS.getWorkspace) {
      getWorkspaceCalls += 1;
      return structuredClone(workspaceWithState(serverState)) as T;
    }
    if (command === MOSS_COMMANDS.cancelRun) {
      serverState = 'cancel_requested';
      return structuredClone(workspaceWithState(serverState)) as T;
    }
    throw new Error(`Unexpected command: ${command}`);
  });

  let hook!: ReturnType<typeof useMossWorkspace>;
  function Harness() {
    hook = useMossWorkspace({ meetingId: 'meeting-1', enabled: true, active: true, service });
    return null;
  }

  let renderer: TestRenderer.ReactTestRenderer | null = null;
  try {
    await act(async () => {
      renderer = TestRenderer.create(<Harness />);
      await Promise.resolve();
    });
    assert.equal(getWorkspaceCalls, 1);
    assert.equal(hook.workspace?.runs[0].state, 'running');
    assert.deepEqual(fakeWindow.timers.delays(), [1500], 'ordinary running must retain the normal interval');

    await act(async () => {
      await hook.cancelRun(hook.workspace!.runs[0].runId);
      await Promise.resolve();
    });

    assert.equal(hook.workspace?.runs[0].state, 'cancel_requested');
    assert.equal(getWorkspaceCalls, 2, 'cancel_requested must refresh immediately instead of waiting 1500ms');
    assert.deepEqual(fakeWindow.timers.delays(), [100], 'cancel_requested must use the fast interval');
  } finally {
    if (renderer) act(() => renderer!.unmount());
    fakeWindow.restore();
  }
});

test('fast cancellation polling stops as soon as the helper reaches a terminal state', async () => {
  const fakeWindow = installFakeWindow();
  let serverState: MossRunState = 'cancel_requested';
  let calls = 0;
  const service = new MossReviewService(async <T,>(command: string) => {
    if (command !== MOSS_COMMANDS.getWorkspace) throw new Error(`Unexpected command: ${command}`);
    calls += 1;
    return structuredClone(workspaceWithState(serverState)) as T;
  });
  let hook!: ReturnType<typeof useMossWorkspace>;
  function Harness() {
    hook = useMossWorkspace({ meetingId: 'meeting-1', enabled: true, active: true, service });
    return null;
  }

  let renderer: TestRenderer.ReactTestRenderer | null = null;
  try {
    await act(async () => {
      renderer = TestRenderer.create(<Harness />);
      await flushMicrotasks();
    });
    assert.equal(calls, 2, 'initial cancel_requested state must trigger one immediate follow-up read');
    assert.equal(hook.workspace?.runs[0].state, 'cancel_requested');
    assert.deepEqual(fakeWindow.timers.delays(), [100]);

    serverState = 'cancelled';
    await act(async () => {
      fakeWindow.timers.fireNext();
      await flushMicrotasks();
    });
    assert.equal(calls, 3);
    assert.equal(hook.workspace?.runs[0].state, 'cancelled');
    assert.deepEqual(fakeWindow.timers.delays(), [], 'terminal state must stop all polling');
  } finally {
    if (renderer) act(() => renderer!.unmount());
    fakeWindow.restore();
  }
});

test('unmount clears the pending ordinary poll and performs no later refresh', async () => {
  const fakeWindow = installFakeWindow();
  let calls = 0;
  const service = new MossReviewService(async <T,>(command: string) => {
    if (command !== MOSS_COMMANDS.getWorkspace) throw new Error(`Unexpected command: ${command}`);
    calls += 1;
    return structuredClone(workspaceWithState('running')) as T;
  });
  function Harness() {
    useMossWorkspace({ meetingId: 'meeting-1', enabled: true, active: true, service });
    return null;
  }

  let renderer: TestRenderer.ReactTestRenderer | null = null;
  try {
    await act(async () => {
      renderer = TestRenderer.create(<Harness />);
      await flushMicrotasks();
    });
    assert.equal(calls, 1);
    assert.deepEqual(fakeWindow.timers.delays(), [1500]);
    act(() => renderer!.unmount());
    renderer = null;
    assert.deepEqual(fakeWindow.timers.delays(), [], 'component cleanup must clear its pending poll');
    assert.equal(calls, 1);
  } finally {
    if (renderer) act(() => renderer!.unmount());
    fakeWindow.restore();
  }
});

test('a poll started before cancellation cannot overwrite the newer cancel_requested response', async () => {
  const fakeWindow = installFakeWindow();
  const staleRunning = deferred<MossWorkspace>();
  let getWorkspaceCalls = 0;
  let serverState: MossRunState = 'running';
  const service = new MossReviewService(async <T,>(command: string) => {
    if (command === MOSS_COMMANDS.getWorkspace) {
      getWorkspaceCalls += 1;
      if (getWorkspaceCalls === 2) return await staleRunning.promise as T;
      return structuredClone(workspaceWithState(serverState)) as T;
    }
    if (command === MOSS_COMMANDS.cancelRun) {
      serverState = 'cancel_requested';
      return structuredClone(workspaceWithState(serverState)) as T;
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  let hook!: ReturnType<typeof useMossWorkspace>;
  function Harness() {
    hook = useMossWorkspace({ meetingId: 'meeting-1', enabled: true, active: true, service });
    return null;
  }

  let renderer: TestRenderer.ReactTestRenderer | null = null;
  try {
    await act(async () => {
      renderer = TestRenderer.create(<Harness />);
      await flushMicrotasks();
    });
    assert.deepEqual(fakeWindow.timers.delays(), [1500]);

    act(() => { fakeWindow.timers.fireNext(); });
    assert.equal(getWorkspaceCalls, 2, 'ordinary poll must be in flight before cancellation');
    await act(async () => {
      await hook.cancelRun(hook.workspace!.runs[0].runId);
      await flushMicrotasks();
    });
    assert.equal(hook.workspace?.runs[0].state, 'cancel_requested');
    assert.equal(getWorkspaceCalls, 3, 'new cancellation cycle must start its immediate refresh');

    await act(async () => {
      staleRunning.resolve(structuredClone(workspaceWithState('running')));
      await flushMicrotasks();
    });
    assert.equal(hook.workspace?.runs[0].state, 'cancel_requested');
    assert.deepEqual(fakeWindow.timers.delays(), [100]);
  } finally {
    if (renderer) act(() => renderer!.unmount());
    fakeWindow.restore();
  }
});
