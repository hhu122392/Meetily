import assert from 'node:assert/strict';
import test from 'node:test';
import {
  APP_NAVIGATION_REQUEST_EVENT,
  requestAppNavigation,
  type AppNavigationRequestDetail,
} from '../../src/lib/navigation-guard';

test('navigation proceeds immediately when no editor guard cancels it', () => {
  const originalWindow = globalThis.window;
  const target = new EventTarget();
  Object.defineProperty(globalThis, 'window', { value: target, configurable: true });
  let proceeded = 0;
  try {
    assert.equal(requestAppNavigation('/settings', () => { proceeded += 1; }), true);
    assert.equal(proceeded, 1);
  } finally {
    Object.defineProperty(globalThis, 'window', { value: originalWindow, configurable: true });
  }
});

test('navigation-specific side effects are postponed until the dirty editor confirms', () => {
  const originalWindow = globalThis.window;
  const target = new EventTarget();
  Object.defineProperty(globalThis, 'window', { value: target, configurable: true });
  let captured: AppNavigationRequestDetail | null = null;
  let proceeded = 0;
  target.addEventListener(APP_NAVIGATION_REQUEST_EVENT, (rawEvent) => {
    const event = rawEvent as CustomEvent<AppNavigationRequestDetail>;
    captured = event.detail;
    event.preventDefault();
  });
  try {
    assert.equal(requestAppNavigation('/meeting-details?id=1', () => { proceeded += 1; }), false);
    assert.equal(proceeded, 0);
    const navigationRequest = captured as AppNavigationRequestDetail | null;
    assert.ok(navigationRequest);
    assert.equal(navigationRequest.destination, '/meeting-details?id=1');
    navigationRequest.proceed();
    assert.equal(proceeded, 1);
  } finally {
    Object.defineProperty(globalThis, 'window', { value: originalWindow, configurable: true });
  }
});
