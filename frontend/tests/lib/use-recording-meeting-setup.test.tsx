import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import '../../src/i18n';
import { useRecordingMeetingSetup, type RecordingMeetingSetupState } from '../../src/hooks/useRecordingMeetingSetup';
import { templateService } from '../../src/services/templateService';
import { createTemplateDraft, editorDraftToTemplate } from '../../src/lib/template-editor';
import type { TemplateDetails } from '../../src/types/summary-template';

function details(id: string, hasProfile = true): TemplateDetails {
  const draft = createTemplateDraft('2026-10-03T00:00:00Z');
  const template = editorDraftToTemplate({ ...draft, id, name: id, description: 'Synthetic test' });
  template.extensions = hasProfile ? { meetily_meeting_context: {
    schema_version: 1, fixed_meeting_mechanism: null, terms: [],
    people: ['a', 'b', 'c'].map(person => ({
      person_id: id + '_' + person, display_name: person, aliases: [],
      department: null, role: null, enabled: true,
    })),
  } } : {};
  return { template, origin: 'custom', schemaVersionOnDisk: 2,
    fileSha256: 'a'.repeat(64), semanticSha256: 'b'.repeat(64),
    isDefault: false, overridesBuiltin: false, readOnly: false };
}

async function withSetup(
  run: (state: () => RecordingMeetingSetupState) => Promise<void>,
  get: typeof templateService.get = async ({ templateId }) => details(templateId, templateId !== 'no_profile'),
  beforeLoaded?: (state: () => RecordingMeetingSetupState) => Promise<void>,
) {
  const original = { list: templateService.list, get: templateService.get, getDefault: templateService.getDefault };
  templateService.list = async () => ({ templates: [], diagnostics: [], deletedTemplates: [], defaultTemplateId: 'default' });
  templateService.get = get;
  templateService.getDefault = async () => ({ templateId: 'default', resolvedTemplateId: 'default', resolutionSource: 'user_default' });
  let state!: RecordingMeetingSetupState;
  let renderer!: TestRenderer.ReactTestRenderer;
  function Probe() { state = useRecordingMeetingSetup(); return null; }
  try {
    await act(async () => { renderer = TestRenderer.create(React.createElement(Probe)); });
    if (beforeLoaded) await beforeLoaded(() => state);
    await act(async () => {
      const deadline = Date.now() + 3000;
      while (state.isLoading && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 1));
    });
    assert.equal(state.isLoading, false);
    assert.ok(state.details);
    await run(() => state);
  } finally {
    if (renderer) act(() => renderer.unmount());
    Object.assign(templateService, original);
  }
}

test('default attendance is submitted without manual customization', async () => {
  await withSetup(async state => {
    assert.equal(state().isCustomized, false);
    const metadata = await state().prepareRecordingMetadata();
    assert.deepEqual(metadata.meetingContextDraft?.attendance.map(p => p.attendance), ['attending', 'attending', 'attending']);
    assert.equal(metadata.templateSelection?.templateId, 'default');
    assert.equal(metadata.meetingContextDraft?.expectedProfileSha256, state().draft?.expectedProfileSha256);
  });
});

test('auto-start waits for the initial draft instead of dropping attendance', async () => {
  let resolveInitial!: (value: TemplateDetails) => void;
  const initial = new Promise<TemplateDetails>(resolve => { resolveInitial = resolve; });
  await withSetup(async () => {}, async () => initial, async state => {
    assert.equal(state().isLoading, true);
    const pending = state().prepareRecordingMetadata();
    resolveInitial(details('default'));
    let metadata!: Awaited<typeof pending>;
    await act(async () => { metadata = await pending; });
    assert.deepEqual(metadata.meetingContextDraft?.attendance.map(p => p.attendance), ['attending', 'attending', 'attending']);
    assert.equal(metadata.templateSelection?.templateId, 'default');
  });
});

test('manual attendance, expected people and reset retain their effective states', async () => {
  await withSetup(async state => {
    act(() => state().updateDraft(d => ({ ...d, attendance: d.attendance.map((p, i) => ({ ...p, attendance: i === 2 ? 'absent' : 'attending' })) })));
    assert.deepEqual((await state().prepareRecordingMetadata()).meetingContextDraft?.attendance.map(p => p.attendance), ['attending', 'attending', 'absent']);
    act(() => state().updateDraft(d => ({ ...d, attendance: d.attendance.map(p => ({ ...p, attendance: 'expected' })) })));
    assert.ok((await state().prepareRecordingMetadata()).meetingContextDraft?.attendance.every(p => p.attendance === 'expected'));
    await act(async () => { await state().resetAdjustments(); });
    assert.equal(state().isCustomized, false);
    assert.ok((await state().prepareRecordingMetadata()).meetingContextDraft?.attendance.every(p => p.attendance === 'attending'));
  });
});

test('templates without a personnel profile still prepare recording', async () => {
  await withSetup(async state => {
    await act(async () => { await state().selectTemplate('no_profile'); });
    const metadata = await state().prepareRecordingMetadata();
    assert.equal(metadata.templateSelection?.templateId, 'no_profile');
    assert.equal(metadata.meetingContextDraft, null);
  });
});

test('host, guests and meeting-only terms are preserved with the selected profile', async () => {
  await withSetup(async state => {
    act(() => state().updateDraft(d => ({ ...d, hostPersonId: 'default_b',
      guests: [{ personId: 'guest_x', displayName: '临时测试人员', aliases: [], department: null, role: null }],
      additionalTerms: [{ termId: 'term_x', canonical: '测试项目', aliases: [], category: null }],
    })));
    const metadata = await state().prepareRecordingMetadata();
    assert.equal(metadata.meetingContextDraft?.hostPersonId, 'default_b');
    assert.equal(metadata.meetingContextDraft?.guests[0].personId, 'guest_x');
    assert.equal(metadata.meetingContextDraft?.additionalTerms[0].termId, 'term_x');
    assert.equal(metadata.meetingContextDraft?.expectedProfileSha256, state().draft?.expectedProfileSha256);
  });
});

test('invalid attendance and mismatched profile hashes block recording', async () => {
  await withSetup(async state => {
    act(() => state().updateDraft(d => ({ ...d, attendance: [...d.attendance, d.attendance[0]] })));
    await assert.rejects(state().prepareRecordingMetadata(), /RECORDING_MEETING_CONTEXT_INVALID/);
    await act(async () => { await state().resetAdjustments(); });
    act(() => state().updateDraft(d => ({ ...d, expectedProfileSha256: 'wrong' })));
    await assert.rejects(state().prepareRecordingMetadata(), /RECORDING_MEETING_CONTEXT_INVALID/);
  });
});

test('pending and out-of-order template loads cannot submit another template', async () => {
  let resolveSlow!: (value: TemplateDetails) => void;
  const slow = new Promise<TemplateDetails>(resolve => { resolveSlow = resolve; });
  await withSetup(async state => {
    let pending!: Promise<void>;
    try {
      act(() => { pending = state().selectTemplate('slow'); });
      await assert.rejects(state().prepareRecordingMetadata(), /RECORDING_MEETING_SETUP_LOADING/);
      await act(async () => { await state().selectTemplate('fast'); });
      resolveSlow(details('slow'));
      await act(async () => { await pending; });
      const metadata = await state().prepareRecordingMetadata();
      assert.equal(metadata.templateSelection?.templateId, 'fast');
      assert.ok(metadata.meetingContextDraft?.attendance.every(p => p.personId.startsWith('fast_')));
    } finally {
      resolveSlow(details('slow'));
      await act(async () => { await pending; });
    }
  }, async ({ templateId }) => templateId === 'slow' ? slow : details(templateId));
});

test('failed template selection blocks the old template until a successful retry', async () => {
  await withSetup(async state => {
    await act(async () => { await state().selectTemplate('broken'); });
    assert.equal(state().error, 'RECORDING_TEMPLATE_LOAD_FAILED');
    await assert.rejects(state().prepareRecordingMetadata(), /RECORDING_MEETING_SETUP_UNAVAILABLE/);
    await act(async () => { await state().selectTemplate('retry'); });
    assert.equal((await state().prepareRecordingMetadata()).templateSelection?.templateId, 'retry');
  }, async ({ templateId }) => {
    if (templateId === 'broken') throw new Error('Synthetic load failure');
    return details(templateId);
  });
});
