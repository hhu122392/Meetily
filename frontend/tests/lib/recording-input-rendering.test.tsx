import assert from 'node:assert/strict';
import test from 'node:test';
import { webcrypto } from 'node:crypto';
import { setImmediate, setTimeout } from 'node:timers/promises';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { emit } from '@tauri-apps/api/event';
import { AppRouterContext, type AppRouterInstance } from 'next/dist/shared/lib/app-router-context.shared-runtime';
import { RecordingStateProvider } from '../../src/contexts/RecordingStateContext';
import { RecordingStatusBar } from '../../src/components/RecordingStatusBar';
import { RecordingInputMonitor } from '../../src/components/RecordingInputMonitor';
import type { RecordingState, RecordingRouteState } from '../../src/services/recordingService';
import { i18n } from '../../src/i18n';

test('real recording surfaces share RMS and poll freshness through pause, resume and remount', async t => {
  const originalWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
  const originalReact = Object.getOwnPropertyDescriptor(globalThis, 'React');
  // tsx uses classic JSX for this Next.js project's jsx: preserve setting.
  Object.defineProperty(globalThis, 'React', { configurable: true, value: React });
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { crypto: webcrypto } });
  const route = (device: string): RecordingRouteState => ({
    active: true, device_name: device, native_id: device, callback_count: 4,
    sample_count: 4800, rms_level: 0.04, peak_level: 0.3,
  });
  const backend: RecordingState = {
    is_recording: true, is_paused: false, is_active: true, is_reconnecting: false,
    recording_duration: 6, active_duration: 6, recording_mode: 'microphone_and_system',
    device_epoch: 1, system_stream_started_qpc_ns: 1, last_error: null,
    cutover: {
      watermark_qpc_ns: null, callback_drain_deadline_qpc_ns: null,
      in_flight_callback_count: 0, old_epoch_last_capture_qpc_ns: null,
      new_epoch_first_capture_qpc_ns: null, late_callback_dropped_frames: 0,
      attribution_error_frames: 0,
    },
    microphone_route: route('test microphone'), system_route: route('test speakers'),
  };
  const unexpected: string[] = [];
  mockIPC(command => {
    if (command === 'poll_audio_device_events') return null;
    if (command === 'get_recording_state') return structuredClone(backend);
    unexpected.push(command);
    throw new Error(`Unexpected IPC: ${command}`);
  }, { shouldMockEvents: true });
  const router: AppRouterInstance = {
    back() {}, forward() {}, refresh() {}, push() {}, replace() {}, prefetch() {},
  };
  let renderer: TestRenderer.ReactTestRenderer | undefined;
  const content = (monitorKey = 'initial') => (
    <AppRouterContext.Provider value={router}>
      <RecordingStateProvider>
        <RecordingStatusBar />
        <RecordingInputMonitor key={monitorKey} />
      </RecordingStateProvider>
    </AppRouterContext.Provider>
  );
  t.after(async () => {
    try { await act(async () => { renderer?.unmount(); await setImmediate(); }); }
    finally {
      clearMocks();
      if (originalWindow) Object.defineProperty(globalThis, 'window', originalWindow);
      else Reflect.deleteProperty(globalThis, 'window');
      if (originalReact) Object.defineProperty(globalThis, 'React', originalReact);
      else Reflect.deleteProperty(globalThis, 'React');
    }
    assert.deepEqual(unexpected, []);
  });
  await i18n.changeLanguage('zh-CN');
  await act(async () => { renderer = TestRenderer.create(content()); await setImmediate(); });
  const check = (level: number) => {
    const meters = renderer!.root.findAll(node => typeof node.type === 'string' && node.props.role === 'meter');
    assert.equal(meters.length, 4, 'both routes have a meter in both actual components');
    assert.deepEqual(meters.map(node => node.props['aria-valuenow']), [level, level, level, level]);
    for (const meter of meters) {
      assert.ok(meter.props['aria-label']);
      assert.equal(meter.props['aria-valuemin'], 0);
      assert.equal(meter.props['aria-valuemax'], 100);
    }
    const visibleText = renderer!.root.findAll(node => typeof node.type === 'string')
      .flatMap(node => node.children.filter(child => typeof child === 'string')).join(' ');
    assert.doesNotMatch(visibleText, /\d+%/, 'visible percentages must not compete with the shared meter');
  };
  const poll = async (advance: boolean, rms = 0.04, peak = 1) => {
    for (const input of [backend.microphone_route, backend.system_route]) {
      if (advance) input.callback_count++;
      input.rms_level = rms;
      input.peak_level = peak;
    }
    backend.active_duration! += 0.5;
    await act(async () => { await setTimeout(550); });
  };
  check(13);
  await act(async () => { await emit('recording-started', {}); });
  await poll(true);
  check(13); // A peak of 1 does not inflate RMS 0.04.
  await poll(false);
  check(0); // Old non-zero readings cannot imply new sound.
  await act(async () => { renderer!.update(content('remounted')); await setImmediate(); });
  check(0);
  await poll(true, 0);
  check(0);
  await poll(true);
  check(13);
  backend.is_paused = true;
  backend.is_active = false;
  await act(async () => { await emit('recording-paused', {}); });
  check(0); // No isPaused prop: the backend event is authoritative.
  backend.is_paused = false;
  backend.is_active = true;
  await act(async () => { await emit('recording-resumed', {}); });
  await poll(true);
  check(13);
});
