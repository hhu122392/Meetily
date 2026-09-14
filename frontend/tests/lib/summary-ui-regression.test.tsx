import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import { SummaryUpdaterButtonGroup } from '../../src/components/MeetingDetails/SummaryUpdaterButtonGroup';
import { useCopyOperations } from '../../src/hooks/meeting-details/useCopyOperations';
import { i18n } from '../../src/i18n';

// The app uses Next's automatic JSX runtime. The standalone tsx test runner
// transpiles imported client components with the classic runtime, so expose
// the already imported React object for those production modules.
Object.assign(globalThis, { React });

test('summary save and copy buttons invoke exactly one operation per click', async () => {
  await i18n.changeLanguage('en');
  let saveCalls = 0;
  let copyCalls = 0;
  let renderer!: TestRenderer.ReactTestRenderer;
  act(() => {
    renderer = TestRenderer.create(
      <SummaryUpdaterButtonGroup
        isSaving={false}
        isDirty
        onSave={async () => {
          saveCalls += 1;
        }}
        onCopy={async () => {
          copyCalls += 1;
        }}
        hasSummary
      />,
    );
  });

  const buttons = renderer.root.findAllByType('button');
  const save = buttons.find((button) => button.props['aria-label'] === 'Save changes');
  const copy = buttons.find((button) => button.props['aria-label'] === 'Copy summary');
  assert.ok(save);
  assert.ok(copy);
  await act(async () => {
    save.props.onClick();
    await Promise.resolve();
  });
  await act(async () => {
    copy.props.onClick();
    await Promise.resolve();
  });
  assert.equal(saveCalls, 1);
  assert.equal(copyCalls, 1);
  act(() => renderer.unmount());
});

test('summary save button only renders while there are unsaved changes', () => {
  let renderer!: TestRenderer.ReactTestRenderer;
  act(() => {
    renderer = TestRenderer.create(
      <SummaryUpdaterButtonGroup
        isSaving={false}
        isDirty={false}
        onSave={async () => undefined}
        onCopy={async () => undefined}
        hasSummary
      />,
    );
  });
  const cleanButtons = renderer.root.findAllByType('button');
  assert.equal(cleanButtons.some((button) => button.props['aria-label'] === 'Save changes'), false);
  assert.equal(cleanButtons.some((button) => button.props['aria-label'] === 'Copy summary'), true);

  act(() => {
    renderer.update(
      <SummaryUpdaterButtonGroup
        isSaving={false}
        isDirty
        onSave={async () => undefined}
        onCopy={async () => undefined}
        hasSummary
      />,
    );
  });
  const dirtyButtons = renderer.root.findAllByType('button');
  assert.equal(dirtyButtons.some((button) => button.props['aria-label'] === 'Save changes'), true);
  act(() => renderer.unmount());
});

test('copy hook writes only rendered summary markdown and keeps lineage metadata out of clipboard text', async () => {
  await i18n.changeLanguage('en');
  const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  let clipboardText = '';
  Object.defineProperty(globalThis, 'navigator', {
    configurable: true,
    value: {
      clipboard: {
        writeText: async (value: string) => {
          clipboardText = value;
        },
      },
    },
  });

  function CopyHarness() {
    const { handleCopySummary } = useCopyOperations({
      meeting: {
        id: 'meeting-copy-regression',
        title: 'Copy regression',
        created_at: '2026-08-30T00:00:00Z',
      },
      transcripts: [],
      meetingTitle: 'Copy regression',
      aiSummary: {
        markdown: '## Decision\n\nShip the verified change.',
        summaryFreshness: {
          status: 'stale',
          reasons: ['transcript_content_changed'],
        },
        sourceBinding: {
          transcriptSha256: 'a'.repeat(64),
        },
      } as never,
      blockNoteSummaryRef: { current: null },
    });
    return <button type="button" onClick={handleCopySummary}>Copy</button>;
  }

  let renderer!: TestRenderer.ReactTestRenderer;
  try {
    act(() => {
      renderer = TestRenderer.create(<CopyHarness />);
    });
    const copy = renderer.root.findByType('button');
    await act(async () => {
      await copy.props.onClick();
    });
    assert.match(clipboardText, /Copy regression/);
    assert.match(clipboardText, /## Decision/);
    assert.match(clipboardText, /Ship the verified change\./);
    assert.doesNotMatch(clipboardText, /summaryFreshness|transcriptSha256|a{64}/);
  } finally {
    if (renderer) act(() => renderer.unmount());
    if (originalNavigator) {
      Object.defineProperty(globalThis, 'navigator', originalNavigator);
    } else {
      Reflect.deleteProperty(globalThis, 'navigator');
    }
  }
});
