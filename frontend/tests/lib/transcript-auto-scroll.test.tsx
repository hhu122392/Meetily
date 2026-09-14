import assert from 'node:assert/strict';
import test from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import { useAutoScroll } from '../../src/hooks/useAutoScroll';

test('follows newly grown content, but respects manual scroll and read-only views', async t => {
  // Only the browser boundary is simulated; run the actual hook and React effects.
  const element = Object.assign(new EventTarget(), { scrollTop: 0, scrollHeight: 400, clientHeight: 400 });
  const scrollRef = { current: element as unknown as HTMLDivElement };
  function View({ count, disabled = false }: { count: number; disabled?: boolean }) {
    useAutoScroll({ scrollRef, segments: Array(count).fill({}), isRecording: true,
      isPaused: false, disableAutoScroll: disabled });
    return null;
  }
  let renderer: TestRenderer.ReactTestRenderer;
  act(() => { renderer = TestRenderer.create(<View count={0} />); });
  t.after(() => act(() => renderer.unmount()));
  element.scrollHeight = 700;
  act(() => renderer.update(<View count={1} />));
  assert.equal(element.scrollTop, 700, 'new content exceeding 100px must still follow the previous bottom');
  await delay(170); // Let the hook finish its own programmatic scroll.
  act(() => {
    element.scrollTop = 0;
    element.dispatchEvent(new Event('scroll'));
    element.scrollHeight = 750;
    renderer.update(<View count={2} />);
  });
  assert.equal(element.scrollTop, 0, 'manual upward scroll must be respected immediately');
  act(() => {
    element.scrollTop = 350;
    element.dispatchEvent(new Event('scroll'));
    element.scrollHeight = 1000;
    renderer.update(<View count={3} />);
  });
  assert.equal(element.scrollTop, 1000, 'returning to the bottom enables following again');
  await delay(170);
  element.scrollTop = 600;
  element.scrollHeight = 1300;
  act(() => renderer.update(<View count={4} disabled />));
  assert.equal(element.scrollTop, 600, 'meeting details must not auto-scroll');
});

test('follows actual content height changes without new segments, but not while reading above', async t => {
  const originalObserver = Object.getOwnPropertyDescriptor(globalThis, 'ResizeObserver');
  let resize: (() => void) | undefined;
  class BrowserObserver {
    constructor(callback: () => void) { resize = callback; }
    observe() {}
    disconnect() { resize = undefined; }
  }
  Object.defineProperty(globalThis, 'ResizeObserver', { configurable: true, value: BrowserObserver });
  const element = Object.assign(new EventTarget(), {
    scrollTop: 300, scrollHeight: 700, clientHeight: 400, lastElementChild: {},
  });
  const scrollRef = { current: element as unknown as HTMLDivElement };
  function View() {
    useAutoScroll({ scrollRef, segments: [{}], isRecording: true, isPaused: false });
    return null;
  }
  let renderer: TestRenderer.ReactTestRenderer;
  act(() => { renderer = TestRenderer.create(<View />); });
  t.after(() => {
    act(() => renderer.unmount());
    if (originalObserver) Object.defineProperty(globalThis, 'ResizeObserver', originalObserver);
    else Reflect.deleteProperty(globalThis, 'ResizeObserver');
  });
  element.scrollHeight = 726;
  act(() => resize?.());
  assert.equal(element.scrollTop, 726, 'a wrapped streaming line must keep the monitor fully visible');
  await delay(170);
  act(() => {
    element.scrollTop = 0;
    element.dispatchEvent(new Event('scroll'));
    element.scrollHeight = 750;
    resize?.();
  });
  assert.equal(element.scrollTop, 0, 'a height update must not take over manual upward scrolling');
});
