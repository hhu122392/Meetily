import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import i18next from 'i18next';
import { resources } from '../../src/i18n/resources';
import { MOSS_COMMANDS } from '../../src/features/moss/service';

const frontendRoot = process.cwd();

function read(relativePath: string): string {
  return fs.readFileSync(path.join(frontendRoot, relativePath), 'utf8');
}

function flatten(value: Record<string, unknown>, prefix = '', result = new Map<string, string>()) {
  for (const [key, child] of Object.entries(value)) {
    const next = prefix ? `${prefix}.${key}` : key;
    if (child && typeof child === 'object' && !Array.isArray(child)) {
      flatten(child as Record<string, unknown>, next, result);
    } else {
      assert.equal(typeof child, 'string', `${next} must be a string`);
      result.set(next, child as string);
    }
  }
  return result;
}

function placeholders(value: string): string[] {
  return [...value.matchAll(/{{\s*([^},\s]+)[^}]*}}/g)].map((match) => match[1]).sort();
}

test('English and Simplified Chinese MOSS resources have exact keys and placeholders', () => {
  const en = flatten(resources.en.moss);
  const zhCN = flatten(resources['zh-CN'].moss);
  assert.deepEqual([...zhCN.keys()].sort(), [...en.keys()].sort());
  assert.ok(en.size >= 100, `expected broad P4 coverage, got ${en.size}`);
  for (const [key, enValue] of en) {
    const zhValue = zhCN.get(key);
    assert.ok(zhValue?.trim(), `${key} must have a Chinese value`);
    assert.deepEqual(placeholders(zhValue!), placeholders(enValue), key);
    assert.doesNotMatch(zhValue!, /\b(?:TODO|TBD|TRANSLATE_ME)\b/i);
  }
});

test('critical boundary text switches locale without changing review state', async () => {
  const instance = i18next.createInstance();
  await instance.init({
    resources,
    lng: 'en',
    fallbackLng: 'en',
    supportedLngs: ['en', 'zh-CN'],
    ns: ['moss'],
    defaultNS: 'moss',
    initAsync: false,
  });
  const state = { meetingId: 'meeting-1', runId: 'run-1', revision: 3, speaker: 'S01' };
  assert.equal(instance.t('moss:settings.nativeHotwordsUnsupported'), 'Not supported');
  assert.match(instance.t('moss:errors.integrationUnavailable'), /MOSS command integration is unavailable/);
  await instance.changeLanguage('zh-CN');
  assert.equal(instance.t('moss:settings.nativeHotwordsUnsupported'), '不支持');
  assert.match(instance.t('moss:errors.integrationUnavailable'), /无法使用 MOSS 命令集/);
  assert.deepEqual(state, { meetingId: 'meeting-1', runId: 'run-1', revision: 3, speaker: 'S01' });
});

test('MOSS is embedded in the existing retranscription dialog and never added to summary models', () => {
  const retranscribe = read('src/components/MeetingDetails/RetranscribeDialog.tsx');
  const transcriptButtons = read('src/components/MeetingDetails/TranscriptButtonGroup.tsx');
  const workspace = read('src/features/moss/components/MossReviewWorkspace.tsx');
  assert.match(retranscribe, /MossReviewWorkspace/);
  assert.match(retranscribe, /standardRetranscriptionEnabled/);
  assert.match(transcriptButtons, /betaFeatures\.moss_post_meeting_enhancement/);
  assert.match(
    transcriptButtons,
    /standardRetranscriptionEnabled=\{betaFeatures\.importAndRetranscribe && Boolean\(meetingFolderPath\)\}/,
  );
  assert.doesNotMatch(workspace, /components\/ui\/dialog|<Dialog\b/);

  for (const source of [
    'src/components/SummaryModelSettings.tsx',
    'src/components/ModelSettingsModal.tsx',
    'src/hooks/meeting-details/useSummaryGeneration.ts',
  ]) {
    assert.doesNotMatch(read(source), /MOSS/i, source);
  }
});

test('review UI exposes every P4 action without automatic name or term guessing', () => {
  const comparison = read('src/features/moss/components/MossTranscriptComparison.tsx');
  const speakers = read('src/features/moss/components/MossSpeakerBindings.tsx');
  const corrections = read('src/features/moss/components/MossTermCorrections.tsx');
  const activation = read('src/features/moss/components/MossActivationPanel.tsx');
  assert.match(comparison, /onSaveSegment/);
  assert.match(speakers, /onSaveBinding/);
  assert.match(speakers, /onSaveOverride/);
  assert.match(corrections, /originalText/);
  assert.match(corrections, /correctedText/);
  assert.match(corrections, /ruleId/);
  assert.match(corrections, /onSetCorrectionState/);
  assert.match(activation, /onActivate/);
  assert.match(activation, /onRollback/);
  assert.doesNotMatch(speakers, /alias|fuzzy|guess|matchName/i);
  assert.doesNotMatch(corrections, /replaceAll|RegExp|fuzzy|guess/i);
});

test('visible MOSS surfaces use controlled errors and accessible live state', () => {
  const sources = [
    'src/features/moss/components/MossReviewWorkspace.tsx',
    'src/features/moss/components/MossSystemStatusCard.tsx',
    'src/features/moss/components/MossTranscriptComparison.tsx',
    'src/features/moss/components/MossSpeakerBindings.tsx',
    'src/features/moss/components/MossTermCorrections.tsx',
    'src/features/moss/components/MossActivationPanel.tsx',
  ].map(read).join('\n');
  assert.match(sources, /aria-live="polite"/);
  assert.match(sources, /aria-busy=/);
  assert.match(sources, /aria-label=/);
  assert.match(sources, /aria-labelledby="moss-activation-confirmation"/);
  assert.match(sources, /MOSS_ERROR_I18N_KEYS/);
  assert.match(sources, /mossRunFailureI18nKey/);
  assert.doesNotMatch(sources, /error\.message|String\(error\)|toast\.error\(\s*error/i);
  assert.doesNotMatch(sources, /t\(\s*selectedRun\?*\.errorCode|t\(\s*runningTask\.errorCode/);
});

test('frontend command map is registered by the real P3-backed Tauri adapter', () => {
  const service = read('src/features/moss/service.ts');
  const hook = read('src/features/moss/useMossWorkspace.ts');
  const integration = read('../docs/moss/p4-p3-frontend-integration.md');
  const tauriRegistration = read('src-tauri/src/lib.rs');
  const adapter = read('src-tauri/src/moss_review.rs');
  assert.match(service, /older\/mismatched desktop binary/);
  assert.match(integration, /4ecbbad4fdc6b68e10309eb223af7e3e4b839522/);
  assert.match(integration, /没有修改 P3 的 `frontend\/src-tauri\/migrations`/);
  assert.match(adapter, /MossCandidateRepository::/);
  assert.match(adapter, /transcribe_file_with_preparation/);
  for (const command of Object.values(MOSS_COMMANDS)) {
    assert.match(integration, new RegExp(`\\b${command}\\b`), command);
    assert.match(tauriRegistration, new RegExp(`moss_review::${command}\\b`), command);
  }
  assert.doesNotMatch(`${service}\n${hook}`, /localStorage|indexedDB|sessionStorage/);
});
