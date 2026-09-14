import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { execFileSync } from 'node:child_process';

const repoRoot = path.resolve(process.argv[2] || '.');
const i18nRoot = path.join(repoRoot, 'docs', 'i18n');
const baselineRoot = path.join(i18nRoot, 'baseline');
const localeRoot = path.join(baselineRoot, 'locales', 'en');
const catalogPath = path.join(baselineRoot, 'en.catalog.generated.json');
const catalog = JSON.parse(fs.readFileSync(catalogPath, 'utf8'));

fs.mkdirSync(localeRoot, { recursive: true });

function writeJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`, 'utf8');
}

function hash(value) {
  return crypto.createHash('sha1').update(value).digest('hex').slice(0, 7);
}

function toCamel(value) {
  const words = value
    .replace(/\{\{?[^}]+\}\}?/g, ' value ')
    .replace(/&/g, ' and ')
    .replace(/[^A-Za-z0-9]+/g, ' ')
    .trim()
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 10);
  if (words.length === 0) return 'text';
  const [first, ...rest] = words;
  const result = first.toLowerCase() + rest.map((word) => word[0].toUpperCase() + word.slice(1)).join('');
  return /^\d/.test(result) ? `text${result}` : result;
}

const commonKeys = new Map(Object.entries({
  'add': 'common.actions.add',
  'apply': 'common.actions.apply',
  'back': 'common.actions.back',
  'browse': 'common.actions.browse',
  'cancel': 'common.actions.cancel',
  'check again': 'common.actions.checkAgain',
  'close': 'common.actions.close',
  'continue': 'common.actions.continue',
  'copy': 'common.actions.copy',
  'delete': 'common.actions.delete',
  'done': 'common.actions.done',
  'download': 'common.actions.download',
  'edit': 'common.actions.edit',
  'find': 'common.actions.find',
  'finish': 'common.actions.finish',
  'next': 'common.actions.next',
  'open': 'common.actions.open',
  'refresh': 'common.actions.refresh',
  'remove': 'common.actions.remove',
  'retry': 'common.actions.retry',
  'save': 'common.actions.save',
  'search': 'common.actions.search',
  'select': 'common.actions.select',
  'skip': 'common.actions.skip',
  'update': 'common.actions.update',
  'completed': 'common.status.completed',
  'downloading...': 'common.status.downloading',
  'failed': 'common.status.failed',
  'loading...': 'common.status.loading',
  'pending': 'common.status.pending',
  'processing': 'common.status.processing',
  'processing...': 'common.status.processing',
  'ready': 'common.status.ready',
  'saving...': 'common.status.saving',
  'searching...': 'common.status.searching',
  'success!': 'common.status.success',
  'waiting...': 'common.status.waiting',
  'unknown error': 'common.errors.unknown',
}));

const complexNormalizations = new Map([
  ['Downloading gemma3:1b', {
    outputs: [{ key: 'models.status.downloadingModel', value: 'Downloading {{model}}' }],
    variables: { 'hardcoded gemma3:1b': 'model' },
    instruction: 'Pass gemma3:1b as model; keep model IDs out of natural-language resources.',
  }],
  ['Downloading gemma3:1b...', {
    outputs: [{ key: 'models.status.downloadingModelEllipsis', value: 'Downloading {{model}}...' }],
    variables: { 'hardcoded gemma3:1b': 'model' },
    instruction: 'Pass gemma3:1b as model; keep model IDs out of natural-language resources.',
  }],
  ['Supported formats: {getAudioFormatsDisplayList()}', {
    outputs: [{ key: 'import.descriptions.supportedFormats', value: 'Supported formats: {{formats}}' }],
    variables: { 'getAudioFormatsDisplayList()': 'formats' },
    instruction: 'Evaluate the format list before calling t().',
  }],
  ['{selectedBlocks.length} blocks', {
    outputs: [
      { key: 'summary.labels.selectedBlocks_one', value: '{{count}} block' },
      { key: 'summary.labels.selectedBlocks_other', value: '{{count}} blocks' },
    ],
    variables: { 'selectedBlocks.length': 'count' },
    instruction: 'Use i18next plural selection with count.',
  }],
  ["{deviceName} - {isActive ? 'Active' : 'Inactive'}", {
    outputs: [
      { key: 'recording.accessibility.deviceActive', value: '{{deviceName}} - Active' },
      { key: 'recording.accessibility.deviceInactive', value: '{{deviceName}} - Inactive' },
    ],
    variables: { deviceName: 'deviceName', "isActive ? 'Active' : 'Inactive'": 'select-key-by-isActive' },
    instruction: 'Choose the active or inactive key before translation.',
  }],
  ['Import complete! {result.segments_count} segments created.', {
    outputs: [
      { key: 'import.messages.importComplete_one', value: 'Import complete! {{count}} segment created.' },
      { key: 'import.messages.importComplete_other', value: 'Import complete! {{count}} segments created.' },
    ],
    variables: { 'result.segments_count': 'count' },
    instruction: 'Use plural selection with result.segments_count as count.',
  }],
  ['Retranscription complete! {event.payload.segments_count} segments created.', {
    outputs: [
      { key: 'transcription.messages.retranscriptionComplete_one', value: 'Retranscription complete! {{count}} segment created.' },
      { key: 'transcription.messages.retranscriptionComplete_other', value: 'Retranscription complete! {{count}} segments created.' },
    ],
    variables: { 'event.payload.segments_count': 'count' },
    instruction: 'Use plural selection with event.payload.segments_count as count.',
  }],
  ['{selectedModel} is downloading ({status.progress}%). Please wait until download completes.', {
    outputs: [{ key: 'models.status.downloadProgress', value: '{{model}} is downloading ({{progress}}%). Please wait until download completes.' }],
    variables: { selectedModel: 'model', 'status.progress': 'progress' },
    instruction: 'Pass selectedModel as model and status.progress as progress.',
  }],
  ["Summary language: {effectiveLangLabel}{isLocalFallbackLanguage ? ' (saved on this device)' : ''}", {
    outputs: [
      { key: 'summary.accessibility.language', value: 'Summary language: {{language}}' },
      { key: 'summary.accessibility.languageLocalFallback', value: 'Summary language: {{language}} (saved on this device)' },
    ],
    variables: { effectiveLangLabel: 'language', "isLocalFallbackLanguage ? ' (saved on this device)' : ''": 'select-key-by-isLocalFallbackLanguage' },
    instruction: 'Choose the local-fallback key when isLocalFallbackLanguage is true.',
  }],
  ['Progress: {Math.round(getProgress(recommendedModel) || 0)}%', {
    outputs: [{ key: 'models.status.progressPercent', value: 'Progress: {{progress}}%' }],
    variables: { 'Math.round(getProgress(recommendedModel) || 0)': 'progress' },
    instruction: 'Calculate and round progress before calling t().',
  }],
  ["{displayInfo?.icon || '✓'} {displayName} ready!", {
    outputs: [{ key: 'transcription.messages.parakeetModelReady', value: '{{icon}} {{model}} ready!' }],
    variables: { "displayInfo?.icon || '✓'": 'icon', displayName: 'model' },
    instruction: 'Resolve the icon and display name before translation.',
  }],
  ['Default: {labelForCode(pinned)} - click it again to unset. Max 5 quick-switch options.', {
    outputs: [{ key: 'summary.descriptions.defaultLanguage', value: 'Default: {{language}} - click it again to unset. Max 5 quick-switch options.' }],
    variables: { 'labelForCode(pinned)': 'language' },
    instruction: 'Resolve the localized language display name before calling t().',
  }],
  ['Pin {labelForCode(code)} as default', {
    outputs: [{ key: 'summary.accessibility.pinLanguageAsDefault', value: 'Pin {{language}} as default' }],
    variables: { 'labelForCode(code)': 'language' },
    instruction: 'Resolve the localized language display name before calling t().',
  }],
  ['Remove {labelForCode(code)}', {
    outputs: [{ key: 'summary.accessibility.removeLanguage', value: 'Remove {{language}}' }],
    variables: { 'labelForCode(code)': 'language' },
    instruction: 'Resolve the localized language display name before calling t().',
  }],
  ['Unpin {labelForCode(code)} as default', {
    outputs: [{ key: 'summary.accessibility.unpinLanguageAsDefault', value: 'Unpin {{language}} as default' }],
    variables: { 'labelForCode(code)': 'language' },
    instruction: 'Resolve the localized language display name before calling t().',
  }],
  ['A new version ({updateInfo.version}) is available', {
    outputs: [{ key: 'updates.messages.newVersionAvailable', value: 'A new version ({{version}}) is available' }],
    variables: { 'updateInfo.version': 'version' },
    instruction: 'Pass updateInfo.version as version.',
  }],
  ["{getModelIcon(model?.accuracy || 'Good')} {displayName} ready!", {
    outputs: [{ key: 'transcription.messages.whisperModelReady', value: '{{icon}} {{model}} ready!' }],
    variables: { "getModelIcon(model?.accuracy || 'Good')": 'icon', displayName: 'model' },
    instruction: 'Resolve the icon and display name before translation.',
  }],
  ["Failed to {isRegeneration ? 'regenerate' : 'generate'} summary", {
    outputs: [
      { key: 'summary.errors.generateFailed', value: 'Failed to generate summary' },
      { key: 'summary.errors.regenerateFailed', value: 'Failed to regenerate summary' },
    ],
    variables: { "isRegeneration ? 'regenerate' : 'generate'": 'select-key-by-isRegeneration' },
    instruction: 'Choose a key using isRegeneration; do not interpolate a verb.',
  }],
  ['Using {modelConfig.provider}/{modelConfig.model}', {
    outputs: [{ key: 'summary.status.usingModel', value: 'Using {{provider}}/{{model}}' }],
    variables: { 'modelConfig.provider': 'provider', 'modelConfig.model': 'model' },
    instruction: 'Pass provider and model as simple variables.',
  }],
  ["{isRegeneration ? 'Regenerating' : 'Generating'} summary...", {
    outputs: [
      { key: 'summary.status.generating', value: 'Generating summary...' },
      { key: 'summary.status.regenerating', value: 'Regenerating summary...' },
    ],
    variables: { "isRegeneration ? 'Regenerating' : 'Generating'": 'select-key-by-isRegeneration' },
    instruction: 'Choose a key using isRegeneration; do not interpolate a sentence fragment.',
  }],
  ['{freshTranscripts.length} transcript segments saved.', {
    outputs: [
      { key: 'recording.messages.transcriptSegmentsSaved_one', value: '{{count}} transcript segment saved.' },
      { key: 'recording.messages.transcriptSegmentsSaved_other', value: '{{count}} transcript segments saved.' },
    ],
    variables: { 'freshTranscripts.length': 'count' },
    instruction: 'Use i18next plural selection with freshTranscripts.length as count.',
  }],
  ['Audio context is in invalid state: {audioRef.current.state}', {
    outputs: [{ key: 'recording.errors.audioContextInvalidState', value: 'Audio context is in invalid state: {{state}}' }],
    variables: { 'audioRef.current.state': 'state' },
    instruction: 'Read audioRef.current.state before translation and pass it as state.',
  }],
  ['Processing {status.chunks_in_queue} remaining chunks...', {
    outputs: [
      { key: 'recording.status.processingRemainingChunks_one', value: 'Processing {{count}} remaining chunk...' },
      { key: 'recording.status.processingRemainingChunks_other', value: 'Processing {{count}} remaining chunks...' },
    ],
    variables: { 'status.chunks_in_queue': 'count' },
    instruction: 'Use plural selection with status.chunks_in_queue as count.',
  }],
]);

const exactSemanticKeys = new Map([
  ['System Audio', 'recording.labels.systemAudio'],
  ['System Audio:', 'recording.labels.systemAudioWithColon'],
]);

function firstSource(entry) {
  return entry.sources[0] || { file: '', line: null, kind: '' };
}

function manualDisposition(entry) {
  const source = firstSource(entry);
  const text = entry.en;
  if (/BasicBlockNoteTest\.tsx$/.test(source.file)) {
    return { disposition: 'excluded_demo_or_test', include: false, reason: 'Developer-only editor test view.' };
  }
  if (/AnalyticsDataModal\.tsx$/.test(source.file) && /^\{\s*"event"/.test(text)) {
    return { disposition: 'excluded_displayed_code_sample', include: false, reason: 'Displayed analytics JSON example, not natural-language UI copy.' };
  }
  if (source.kind === 'object_property:name') {
    if (/^Auto Detect/.test(text)) {
      return { disposition: 'mapped_special_language_option', include: true, reason: 'Special language option not provided by Intl.DisplayNames.' };
    }
    return { disposition: 'localized_via_intl_display_names', include: false, reason: 'Language display name is generated from its language code.' };
  }
  if (/must be used within/i.test(text)) {
    return { disposition: 'excluded_developer_invariant', include: false, reason: 'React provider invariant intended for developers, not end users.' };
  }
  if (source.kind === 'new_error') {
    return { disposition: 'mapped_structured_frontend_error', include: true, reason: 'Conservatively treated as user-reachable until error-code migration proves otherwise.' };
  }
  if (/^indirect_call:/.test(source.kind)) {
    return { disposition: 'mapped_indirect_ui_state_or_error', include: true, reason: 'Passed to a state setter whose value can be rendered.' };
  }
  return { disposition: 'mapped_after_manual_review', include: true, reason: 'Retained conservatively as user-visible text.' };
}

function domainFor(entry) {
  const source = firstSource(entry).file.toLowerCase();
  const text = entry.en.toLowerCase();
  if (source.includes('analytics')) return 'analytics';
  if (source.includes('update')) return 'updates';
  if (source.includes('onboarding')) return 'onboarding';
  if (source.includes('importaudio') || source.includes('databaseimport') || source.includes('useimport')) return 'import';
  if (source.includes('summary') || source.includes('aisummary')) return 'summary';
  if (source.includes('modelsettings') || source.includes('builtinmodel') || source.includes('ollama')) return 'models';
  if (source.includes('transcriptrecovery')) return 'meetings';
  if (source.includes('transcript') || source.includes('whisper') || source.includes('parakeet') || source.includes('language') || source.includes('confidence')) return 'transcription';
  if (source.includes('recording') || source.includes('audio') || source.includes('device') || source.includes('permission') || source.includes('bluetooth')) return 'recording';
  if (source.includes('meeting') || source.includes('/notes/')) return 'meetings';
  if (source.includes('sidebar') || source.includes('mainnav') || /^(home|settings|meeting notes|import audio)$/.test(text)) return 'navigation';
  if (source.includes('settings') || source.includes('preference') || source.includes('beta') || source.includes('about') || source.includes('/info.')) return 'settings';
  return 'common';
}

function intentFor(entry) {
  const source = firstSource(entry);
  const text = entry.en.trim();
  const kind = source.kind;
  if (/aria-label/.test(kind)) return 'accessibility';
  if (/placeholder/.test(kind)) return 'placeholders';
  if (/toast\.error|setError|setOpenRouterError|setSummaryError|new_error/.test(kind) || /^(Error|Failed|Unable|Could not|Cannot|Something went wrong)/i.test(text)) return 'errors';
  if (/toast\.success|toast\.info/.test(kind)) return 'messages';
  if (/setStatus/.test(kind) || /^(Loading|Downloading|Processing|Saving|Searching|Waiting|Ready|Completed|Failed|Listening|Recording|Initializing|Flushing)/i.test(text) || /\.\.\.$/.test(text)) return 'status';
  if (/^(Add|Apply|Back|Browse|Cancel|Check|Close|Continue|Copy|Delete|Download|Edit|Enable|Enhance|Find|Finish|Grant|Import|Install|Next|Open|Pause|Refresh|Regenerate|Remove|Resume|Retry|Save|Search|Select|Skip|Start|Stop|Test|Try|Unpin|Update|Yes)(\b|,)/i.test(text)) return 'actions';
  if (/title/.test(kind)) return 'accessibility';
  if (/description/.test(kind) || text.length > 100) return 'descriptions';
  if (/\?$/.test(text)) return 'prompts';
  return 'labels';
}

function normalizeSimplePlaceholders(value) {
  return value.replace(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g, '{{$1}}');
}

function keyFor(entry) {
  const semanticOverride = exactSemanticKeys.get(entry.en.trim());
  if (semanticOverride) return semanticOverride;
  const exact = commonKeys.get(entry.en.trim().toLowerCase());
  if (exact) return exact;
  const domain = domainFor(entry);
  const intent = intentFor(entry);
  return `${domain}.${intent}.${toCamel(entry.en)}`;
}

const formalValues = new Map();
const sourceMap = [];
const dispositions = [];
const placeholderPlans = [];

function registerValue(key, value) {
  let finalKey = key;
  if (formalValues.has(finalKey) && formalValues.get(finalKey) !== value) {
    finalKey = `${key}Variant${hash(value)}`;
  }
  formalValues.set(finalKey, value);
  return finalKey;
}

for (const entry of catalog.entries) {
  if (entry.status === 'translate' || entry.status === 'manual_review') {
    const decision = entry.status === 'translate'
      ? { disposition: 'mapped_confirmed_frontend_text', include: true, reason: 'Confirmed frontend translation candidate.' }
      : manualDisposition(entry);
    const mapping = {
      id: entry.id,
      originalKey: entry.key,
      en: entry.en,
      originalStatus: entry.status,
      disposition: decision.disposition,
      reason: decision.reason,
      finalKeys: [],
      sources: entry.sources,
    };

    if (decision.include) {
      const complex = complexNormalizations.get(entry.en);
      if (complex) {
        for (const output of complex.outputs) mapping.finalKeys.push(registerValue(output.key, output.value));
        placeholderPlans.push({
          id: entry.id,
          sourceText: entry.en,
          finalKeys: mapping.finalKeys,
          variableMap: complex.variables,
          migrationInstruction: complex.instruction,
          sources: entry.sources,
        });
      } else {
        const value = normalizeSimplePlaceholders(entry.en);
        mapping.finalKeys.push(registerValue(keyFor(entry), value));
      }
      sourceMap.push(mapping);
    }
    dispositions.push(mapping);
    continue;
  }

  if (entry.status === 'manual_visibility_review') {
    dispositions.push({
      id: entry.id,
      originalKey: entry.key,
      en: entry.en,
      originalStatus: entry.status,
      disposition: 'classified_in_native_visibility_matrix',
      reason: 'Handled by native-visibility-matrix.json; not included in the frontend locale bundle.',
      finalKeys: [],
      sources: entry.sources,
    });
    continue;
  }

  if (entry.status === 'excluded_legacy_source') {
    dispositions.push({
      id: entry.id,
      originalKey: entry.key,
      en: entry.en,
      originalStatus: entry.status,
      disposition: 'excluded_legacy_source',
      reason: 'Explicitly old/legacy Rust source; retained only for audit traceability.',
      finalKeys: [],
      sources: entry.sources,
    });
    continue;
  }

  dispositions.push({
    id: entry.id,
    originalKey: entry.key,
    en: entry.en,
    originalStatus: entry.status,
    disposition: 'separate_template_content_localization',
    reason: 'Built-in template/AI content is versioned separately from UI locale resources.',
    finalKeys: [],
    sources: entry.sources,
  });
}

function setNested(target, parts, value) {
  let cursor = target;
  for (const part of parts.slice(0, -1)) {
    if (!cursor[part]) cursor[part] = {};
    cursor = cursor[part];
  }
  cursor[parts.at(-1)] = value;
}

const namespaces = {};
for (const [key, value] of [...formalValues.entries()].sort(([a], [b]) => a.localeCompare(b))) {
  const [namespace, ...parts] = key.split('.');
  namespaces[namespace] ||= {};
  setNested(namespaces[namespace], parts, value);
}
for (const [namespace, values] of Object.entries(namespaces)) {
  writeJson(path.join(localeRoot, `${namespace}.json`), values);
}

function readSourceLine(source) {
  const fullPath = path.join(repoRoot, source.file);
  if (!fs.existsSync(fullPath) || !source.line) return '';
  return fs.readFileSync(fullPath, 'utf8').split(/\r?\n/)[source.line - 1] || '';
}

function nativeDecision(entry) {
  const source = firstSource(entry);
  const rawLine = readSourceLine(source);
  if (entry.status === 'excluded_legacy_source') {
    return {
      visibility: 'legacy_excluded',
      surface: 'none',
      phase3Action: 'keep_excluded_or_delete_dead_source',
      evidence: 'Filename is explicitly marked old/legacy.',
    };
  }
  if (/\/tray\.rs$/.test(source.file)) {
    return {
      visibility: 'runtime_user_visible',
      surface: 'native_tray_menu',
      phase3Action: 'translate_in_rust_native_bundle',
      evidence: 'String originates in the active tray menu builder.',
    };
  }
  if (/\/notifications\//.test(source.file) && /(title|body|notification)/i.test(`${rawLine} ${entry.en}`)) {
    return {
      visibility: 'runtime_user_visible',
      surface: 'system_notification_or_notification_command',
      phase3Action: 'translate_in_rust_or_accept_frontend_resolved_key',
      evidence: 'String is in the notification subsystem or its command boundary.',
    };
  }
  if (/userMessage/.test(rawLine)) {
    return {
      visibility: 'runtime_user_visible',
      surface: 'frontend_user_message_from_rust',
      phase3Action: 'replace_with_structured_error_code',
      evidence: 'Rust response explicitly labels the value as userMessage.',
    };
  }
  if (/"message"\s*:/.test(rawLine)) {
    return {
      visibility: 'runtime_frontend_boundary',
      surface: 'command_response_message',
      phase3Action: 'replace_with_status_code_or_translate_in_frontend',
      evidence: 'Value is returned in a command response message field.',
    };
  }
  if (/Err\s*\(|format!\s*\(/.test(rawLine) || /error|failed|cannot|not found|invalid|unable/i.test(entry.en)) {
    return {
      visibility: 'runtime_frontend_boundary',
      surface: 'tauri_command_error_or_propagated_result',
      phase3Action: 'replace_with_structured_error_code',
      evidence: 'Conservatively classified as user-reachable because it is an error/result boundary and logs were excluded by the extractor.',
    };
  }
  return {
    visibility: 'runtime_frontend_boundary',
    surface: 'command_result_or_runtime_status',
    phase3Action: 'replace_with_stable_code_then_translate_at_owning_surface',
    evidence: 'Conservative runtime-boundary classification; candidate is not a developer log under extraction rules.',
  };
}

const nativeEntries = catalog.entries
  .filter((entry) => entry.status === 'manual_visibility_review' || entry.status === 'excluded_legacy_source')
  .map((entry) => ({
    id: entry.id,
    en: entry.en,
    originalStatus: entry.status,
    ...nativeDecision(entry),
    sources: entry.sources,
  }));

const templateEntries = catalog.entries
  .filter((entry) => entry.status === 'separate_content_localization')
  .map((entry) => ({
    id: entry.id,
    en: entry.en,
    disposition: 'localize_as_versioned_template_content',
    targetFields: 'name, description, section headings, explanatory content, or prompt text as indicated by source JSON path',
    sources: entry.sources,
  }));

const doNotTranslate = {
  schemaVersion: 1,
  policy: 'Keep exact terms, protocol identifiers, model IDs, URLs, file extensions, command names, storage keys, and code samples unchanged unless a product owner explicitly approves a localized display alias.',
  exactTerms: [
    'Meetily', 'Whisper', 'Parakeet', 'Ollama', 'OpenAI', 'OpenRouter', 'Claude', 'Groq',
    'Deepgram', 'ElevenLabs', 'FFmpeg', 'BlackHole', 'Tauri', 'Next.js', 'React',
    'i18next', 'API', 'API Key', 'URL', 'JSON', 'Markdown', 'GitHub', 'macOS', 'Windows', 'Linux',
  ],
  fileExtensions: ['MP3', 'WAV', 'MP4', 'FLAC', 'OGG', 'MKV', 'WebM', 'WMA', 'JSON', 'GGUF'],
  codeAndProtocolPatterns: [
    'Tauri command IDs such as set_language_preference',
    'event IDs such as request-recording-toggle',
    'storage keys such as primaryLanguage and meetily.uiLocale',
    'model IDs and tags such as gemma3:1b',
    'URLs, filesystem paths, hashes, version strings, MIME types, environment variables, and CLI flags',
  ],
  displayRule: 'A surrounding label or explanation may be translated; the exact protected token remains unchanged.',
};

const glossary = {
  schemaVersion: 1,
  locale: 'zh-CN',
  entries: [
    ['Meeting', '会议', 'Do not use 会面 in the product UI.'],
    ['Meeting Notes', '会议笔记', 'Navigation and saved notes.'],
    ['Recording', '录音', 'Use 录制中 only for an active status.'],
    ['Start recording', '开始录音', 'Action label.'],
    ['Stop recording', '停止录音', 'Action label.'],
    ['Pause recording', '暂停录音', 'Action label.'],
    ['Resume recording', '继续录音', 'Action label.'],
    ['Transcript', '转录文本', 'Use 转录 in compact navigation or as a process noun.'],
    ['Transcription', '转录', 'Do not use 听写.'],
    ['Retranscribe', '重新转录', 'Action.'],
    ['Enhance', '增强转录', 'When the feature specifically reprocesses a transcript.'],
    ['Summary', '摘要', 'AI Summary is AI 摘要.'],
    ['Generate summary', '生成摘要', 'Action.'],
    ['Regenerate summary', '重新生成摘要', 'Action.'],
    ['Action Items', '行动项', 'Team collaboration context.'],
    ['Key Points', '要点', 'Summary section.'],
    ['Decisions', '决策', 'Summary section.'],
    ['Main Topics', '主要议题', 'Summary section.'],
    ['Speaker', '发言人', 'Never translate as 扬声器.'],
    ['Microphone', '麦克风', 'Audio input device.'],
    ['System Audio', '系统音频', 'Computer playback audio.'],
    ['Audio Device', '音频设备', 'Generic device term.'],
    ['Model', '模型', 'AI or transcription model; retain the actual model name.'],
    ['Provider', '服务提供方', 'Use 服务商 only if space is constrained and approved.'],
    ['Built-in AI', '内置 AI', 'Keep AI uppercase.'],
    ['Template', '模板', 'Summary template.'],
    ['Language', '语言', 'Qualify as 显示语言、转录语言、摘要语言 when ambiguity exists.'],
    ['Display Language', '显示语言', 'Controls UI only.'],
    ['Transcription Language', '转录语言', 'Controls speech recognition.'],
    ['Summary Language', '摘要语言', 'Controls generated summary output.'],
    ['Auto Detect', '自动检测', 'Special language option.'],
    ['Auto Detect (Original Language)', '自动检测（保留原语言）', 'Do not imply translation.'],
    ['Auto Detect (Translate to English)', '自动检测（翻译为英语）', 'Explicitly indicates English output.'],
    ['Confidence', '置信度', 'Explain as recognition confidence in help text.'],
    ['Settings', '设置', 'Navigation and page title.'],
    ['Preferences', '偏好设置', 'User preferences section.'],
    ['Onboarding', '初始设置', 'Do not expose the engineering term 引导流程 to users.'],
    ['Import Audio', '导入音频', 'Action and navigation label.'],
    ['Release Notes', '更新说明', 'Prefer over 发行说明.'],
    ['Update Available', '有可用更新', 'Status title.'],
    ['Beta Features', '测试功能', 'If retaining Beta as a brand tone, use Beta 功能 consistently.'],
    ['Analytics', '使用情况分析', 'Privacy-sensitive feature; avoid ambiguous 数据分析.'],
    ['Privacy', '隐私', 'Legal/privacy context requires human review.'],
    ['Download', '下载', 'Action.'],
    ['Install', '安装', 'Action.'],
    ['Retry', '重试', 'Action.'],
    ['Cancel', '取消', 'Action.'],
    ['Delete', '删除', 'Destructive action; confirmation must state the target.'],
  ].map(([en, zhCN, note]) => ({ en, zhCN, note, approved: true })),
};

const placeholderInventory = [...formalValues.entries()]
  .flatMap(([key, value]) => {
    const variables = [...value.matchAll(/\{\{([A-Za-z_][A-Za-z0-9_]*)\}\}/g)].map((match) => match[1]);
    return variables.length ? [{ key, en: value, variables }] : [];
  })
  .sort((a, b) => a.key.localeCompare(b.key));

const manifest = {
  schemaVersion: 1,
  phase: '15.3 / Phase 0 - Freeze English baseline',
  generatedAt: new Date().toISOString(),
  sourceCommit: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: repoRoot, encoding: 'utf8' }).trim(),
  counts: {
    catalogEntries: catalog.entries.length,
    confirmedFrontendEntries: catalog.entries.filter((entry) => entry.status === 'translate').length,
    manualFrontendReviewEntries: catalog.entries.filter((entry) => entry.status === 'manual_review').length,
    nativeReviewEntries: catalog.entries.filter((entry) => entry.status === 'manual_visibility_review').length,
    legacyEntries: catalog.entries.filter((entry) => entry.status === 'excluded_legacy_source').length,
    templateContentEntries: catalog.entries.filter((entry) => entry.status === 'separate_content_localization').length,
    formalSourceMappings: sourceMap.length,
    formalTranslationKeys: formalValues.size,
    namespaces: Object.keys(namespaces).length,
    complexPlaceholderPlans: placeholderPlans.length,
    allPlaceholderKeys: placeholderInventory.length,
  },
  namespaces: Object.keys(namespaces).sort(),
  generatedFiles: [
    'baseline/locales/en/*.json',
    'baseline/source-map.json',
    'baseline/disposition.json',
    'baseline/placeholder-migration.json',
    'baseline/placeholders.json',
    'baseline/native-visibility-matrix.json',
    'baseline/template-content-inventory.json',
    'baseline/do-not-translate.json',
    'baseline/glossary.en-zh-CN.json',
  ],
};

writeJson(path.join(baselineRoot, 'source-map.json'), { schemaVersion: 1, entries: sourceMap });
writeJson(path.join(baselineRoot, 'disposition.json'), { schemaVersion: 1, entries: dispositions });
writeJson(path.join(baselineRoot, 'placeholder-migration.json'), { schemaVersion: 1, complexSourceExpressions: placeholderPlans });
writeJson(path.join(baselineRoot, 'placeholders.json'), { schemaVersion: 1, entries: placeholderInventory });
writeJson(path.join(baselineRoot, 'native-visibility-matrix.json'), { schemaVersion: 1, entries: nativeEntries });
writeJson(path.join(baselineRoot, 'template-content-inventory.json'), { schemaVersion: 1, entries: templateEntries });
writeJson(path.join(baselineRoot, 'do-not-translate.json'), doNotTranslate);
writeJson(path.join(baselineRoot, 'glossary.en-zh-CN.json'), glossary);
writeJson(path.join(baselineRoot, 'phase0.manifest.json'), manifest);

console.log(JSON.stringify(manifest.counts, null, 2));
