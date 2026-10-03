import assert from 'node:assert/strict';
import test from 'node:test';
import { webcrypto } from 'node:crypto';
import React, { useEffect, useState } from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { usePaginatedTranscripts } from '../../src/hooks/usePaginatedTranscripts';

test('refreshing corrected transcripts keeps the loaded summary editor mounted', async () => {
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { crypto: webcrypto } });
  let releaseMetadata!: () => void;
  let refreshing = false;
  let mounts = 0;
  let changeBody!: (body: string) => void;
  let state!: ReturnType<typeof usePaginatedTranscripts>;
  let renderer!: TestRenderer.ReactTestRenderer;
  mockIPC(async command => {
    if (command === 'api_get_meeting_metadata') {
      if (refreshing) await new Promise<void>(resolve => { releaseMetadata = resolve; });
      return { id: 'virtual-meeting', title: 'Virtual meeting' };
    }
    if (command === 'api_get_meeting_transcripts') return {
      transcripts: [{ id: 'virtual-segment', text: refreshing ? 'corrected source' : 'original source' }],
      has_more: false, total_count: 1,
    };
    throw new Error(command);
  });
  function Editor() {
    const [body, setBody] = useState('saved summary');
    changeBody = setBody;
    useEffect(() => { mounts++; }, []);
    return <p>{body}</p>;
  }
  function Page() {
    state = usePaginatedTranscripts({ meetingId: 'virtual-meeting' });
    return state.isLoading || !state.metadata ? <span>loading</span> : <Editor />;
  }
  try {
    await act(async () => { renderer = TestRenderer.create(<Page />); });
    act(() => changeBody('restored historical summary, unsaved edit'));
    refreshing = true;
    let pending!: Promise<void>;
    await act(async () => { pending = state.refetch(); });
    assert.equal(state.isLoading, false, 'refresh must not replace the page with the initial loader');
    assert.equal(state.metadata?.id, 'virtual-meeting');
    assert.equal(renderer.root.findByType('p').children.join(''), 'restored historical summary, unsaved edit');
    await act(async () => { releaseMetadata(); await pending; });
    assert.equal(mounts, 1);
    assert.equal(state.transcripts[0].text, 'corrected source');
    assert.equal(renderer.root.findByType('p').children.join(''), 'restored historical summary, unsaved edit');
  } finally {
    act(() => renderer?.unmount());
    clearMocks();
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow);
    else Reflect.deleteProperty(globalThis, 'window');
  }
});
