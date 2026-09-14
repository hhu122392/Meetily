import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import { i18n } from '../../src/i18n';
import { MossSystemStatusCard } from '../../src/features/moss/components/MossSystemStatusCard';
import { MossTranscriptComparison } from '../../src/features/moss/components/MossTranscriptComparison';
import { MossActivationPanel } from '../../src/features/moss/components/MossActivationPanel';
import { MossTermCorrections } from '../../src/features/moss/components/MossTermCorrections';
import { MossReviewService } from '../../src/features/moss/service';
import { mossSystemStatusFixture, mossWorkspaceFixture } from './fixtures';

function renderedText(renderer: TestRenderer.ReactTestRenderer): string {
  const values: string[] = [];
  const walk = (value: unknown): void => {
    if (typeof value === 'string') values.push(value);
    else if (Array.isArray(value)) value.forEach(walk);
    else if (value && typeof value === 'object' && 'children' in value) {
      walk((value as { children?: unknown }).children);
    }
  };
  walk(renderer.toJSON());
  return values.join(' ');
}

test('disabled status card performs no backend call and explains the gate', async () => {
  await i18n.changeLanguage('en');
  let calls = 0;
  const service = new MossReviewService(async <T,>() => {
    calls += 1;
    return structuredClone(mossSystemStatusFixture) as T;
  });
  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => {
    renderer = TestRenderer.create(<MossSystemStatusCard enabled={false} service={service} />);
  });
  assert.equal(calls, 0);
  assert.match(renderedText(renderer), /internal MOSS feature is off/i);
  act(() => renderer.unmount());
});

test('enabled status card renders backend facts and the mandatory hotword limitation', async () => {
  await i18n.changeLanguage('en');
  let calls = 0;
  const service = new MossReviewService(async <T,>() => {
    calls += 1;
    return structuredClone(mossSystemStatusFixture) as T;
  });
  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => {
    renderer = TestRenderer.create(<MossSystemStatusCard enabled service={service} />);
  });
  const text = renderedText(renderer);
  assert.equal(calls, 1);
  assert.match(text, /0\.2\.2/);
  assert.match(text, /Intel\(R\) Arc\(TM\) Graphics/);
  assert.match(text, /Native hotwords Not supported/);
  assert.match(text, /does not provide native hotwords/);
  act(() => renderer.unmount());
});

test('status card converts a mismatched desktop command error into safe visible copy', async () => {
  await i18n.changeLanguage('en');
  const service = new MossReviewService(async () => {
    throw new Error('missing command at D:\\MeetilyData\\private\\meeting.wav');
  });
  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => {
    renderer = TestRenderer.create(<MossSystemStatusCard enabled service={service} />);
  });
  const text = renderedText(renderer);
  assert.match(text, /MOSS command integration is unavailable/);
  assert.match(text, /Reference:/);
  assert.doesNotMatch(text, /MeetilyData|meeting\.wav|missing command/i);
  act(() => renderer.unmount());
});

test('comparison component renders current and candidate evidence without merging them', async () => {
  await i18n.changeLanguage('en');
  const review = mossWorkspaceFixture().review!;
  let renderer!: TestRenderer.ReactTestRenderer;
  act(() => {
    renderer = TestRenderer.create(
      <MossTranscriptComparison
        review={review}
        pendingKeys={new Set()}
        onSaveSegment={async () => null}
      />,
    );
  });
  const text = renderedText(renderer);
  assert.match(text, /Current transcript/);
  assert.match(text, /MOSS candidate/);
  assert.match(text, /S01/);
  assert.match(text, /S02/);
  assert.match(text, /现行文字/);
  assert.match(text, /西吉艾斯进度正常/);
  assert.match(text, /1 aligned segments/);
  assert.match(text, /original MOSS segment/);
  assert.match(text, /frozen source transcript/);
  assert.match(text, /Last active audio 0:04/);
  act(() => renderer.unmount());
});

test('candidate editor rejects a whitespace-only no-op and saves normalized candidate text', async () => {
  await i18n.changeLanguage('en');
  const review = mossWorkspaceFixture().review!;
  const saves: Array<{ segmentId: string; text: string }> = [];
  let renderer!: TestRenderer.ReactTestRenderer;
  act(() => {
    renderer = TestRenderer.create(
      <MossTranscriptComparison
        review={review}
        pendingKeys={new Set()}
        onSaveSegment={async (segmentId, text) => {
          saves.push({ segmentId, text });
          return { saved: true };
        }}
      />,
    );
  });
  const firstEdit = renderer.root.findAllByType('button').find(
    (button) => String(button.props['aria-label'] ?? '').startsWith('Edit candidate segment'),
  );
  assert.ok(firstEdit);
  act(() => firstEdit.props.onClick());

  const textarea = renderer.root.findByType('textarea');
  act(() => textarea.props.onChange({ target: { value: `  ${review.candidate.segments[0].text}  ` } }));
  let save = renderer.root.findAllByType('button').find(
    (button) => String(button.props['aria-label'] ?? '').startsWith('Save edit for candidate segment'),
  );
  assert.equal(save?.props.disabled, true);

  act(() => textarea.props.onChange({ target: { value: '  reviewed candidate text  ' } }));
  save = renderer.root.findAllByType('button').find(
    (button) => String(button.props['aria-label'] ?? '').startsWith('Save edit for candidate segment'),
  );
  assert.equal(save?.props.disabled, false);
  await act(async () => { await save!.props.onClick(); });
  assert.deepEqual(saves, [{ segmentId: 'candidate-1', text: 'reviewed candidate text' }]);
  act(() => renderer.unmount());
});

test('term undo and activation confirmation call only their explicit operations', async () => {
  await i18n.changeLanguage('en');
  const review = mossWorkspaceFixture().review!;
  const correctionCalls: Array<{ id: string; applied: boolean }> = [];
  let correctionRenderer!: TestRenderer.ReactTestRenderer;
  act(() => {
    correctionRenderer = TestRenderer.create(
      <MossTermCorrections
        review={review}
        pendingKeys={new Set()}
        onSetCorrectionState={async (id, applied) => {
          correctionCalls.push({ id, applied });
          return { saved: true };
        }}
      />,
    );
  });
  assert.match(renderedText(correctionRenderer), /西吉艾斯进度正常/);
  assert.match(renderedText(correctionRenderer), /CGS 进度正常/);
  assert.match(renderedText(correctionRenderer), /term-cgs-alias-1/);
  await act(async () => { await correctionRenderer.root.findByType('button').props.onClick(); });
  assert.deepEqual(correctionCalls, [{ id: 'correction-1', applied: false }]);
  act(() => correctionRenderer.unmount());

  let activations = 0;
  let activationRenderer!: TestRenderer.ReactTestRenderer;
  act(() => {
    activationRenderer = TestRenderer.create(
      <MossActivationPanel
        review={review}
        pendingKeys={new Set()}
        onActivate={async () => { activations += 1; return { activated: true }; }}
        onRollback={async () => { throw new Error('rollback must not run'); }}
      />,
    );
  });
  act(() => activationRenderer.root.findByType('button').props.onClick());
  assert.match(renderedText(activationRenderer), /Activate this candidate\?/);
  const confirmationButtons = activationRenderer.root.findAllByType('button');
  await act(async () => { await confirmationButtons[1].props.onClick(); });
  assert.equal(activations, 1);
  act(() => activationRenderer.unmount());
});

test('an activated candidate is read-only across text and correction controls', async () => {
  await i18n.changeLanguage('en');
  const review = mossWorkspaceFixture().review!;
  review.candidate.isActive = true;
  review.activation.activeRunId = review.candidate.runId;
  review.activation.activeActivationId = 'activation-1';
  review.activation.canActivate = false;
  review.activation.canRollback = true;

  let comparisonRenderer!: TestRenderer.ReactTestRenderer;
  act(() => {
    comparisonRenderer = TestRenderer.create(
      <MossTranscriptComparison
        review={review}
        pendingKeys={new Set()}
        onSaveSegment={async () => { throw new Error('active candidate must not save'); }}
      />,
    );
  });
  assert.match(renderedText(comparisonRenderer), /activated candidate is read-only/);
  const editButtons = comparisonRenderer.root.findAllByType('button').filter(
    (button) => String(button.props['aria-label'] ?? '').startsWith('Edit candidate segment'),
  );
  assert.ok(editButtons.length > 0);
  assert.ok(editButtons.every((button) => button.props.disabled === true));
  act(() => comparisonRenderer.unmount());

  let correctionRenderer!: TestRenderer.ReactTestRenderer;
  act(() => {
    correctionRenderer = TestRenderer.create(
      <MossTermCorrections
        review={review}
        pendingKeys={new Set()}
        onSetCorrectionState={async () => { throw new Error('active candidate must not change corrections'); }}
      />,
    );
  });
  assert.equal(correctionRenderer.root.findByType('button').props.disabled, true);
  act(() => correctionRenderer.unmount());
});
