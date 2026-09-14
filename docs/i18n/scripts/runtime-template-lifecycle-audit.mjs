#!/usr/bin/env node

import fs from 'node:fs/promises';
import path from 'node:path';

const port = Number(process.argv[2] || 9371);
const root = path.resolve(process.argv[3] || '.');
const runId = `runtime_template_lifecycle_${Date.now()}`;
const workRoot = path.join(root, 'target-phase5-release', 'runtime-template-lifecycle');
const workDirectory = path.join(workRoot, runId);
const evidenceDirectory = path.join(root, 'target', 'release', 'docs', 'phase-0-custom-summary-templates', 'audit');
const reportPath = path.join(evidenceDirectory, 'template-generation-lifecycle-runtime-audit.json');
const historyScreenshotPath = path.join(evidenceDirectory, 'template-generation-history-zh-CN.png');
const cleanupScreenshotPath = path.join(evidenceDirectory, 'template-snapshot-cleanup-preview-zh-CN.png');
const priorAuditMeetingIds = (process.env.P2_300_PRIOR_AUDIT_MEETING_IDS || '')
  .split(',')
  .map((value) => value.trim())
  .filter((value) => /^meeting-[0-9a-f-]{36}$/.test(value));
await fs.mkdir(workDirectory, { recursive: true });
await fs.mkdir(evidenceDirectory, { recursive: true });

async function waitForTarget(timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
      const target = targets.find((candidate) => candidate.type === 'page');
      if (target?.webSocketDebuggerUrl) return target;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`No Tauri page target appeared on port ${port}`);
}

const target = await waitForTarget();
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});

let nextId = 1;
const pending = new Map();
const diagnostics = { exceptions: [], consoleErrors: [], logErrors: [] };
socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    const operation = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) operation.reject(new Error(JSON.stringify(message.error)));
    else operation.resolve(message.result);
    return;
  }
  if (message.method === 'Runtime.exceptionThrown') diagnostics.exceptions.push(message.params);
  if (message.method === 'Runtime.consoleAPICalled' && message.params?.type === 'error') diagnostics.consoleErrors.push(message.params);
  if (message.method === 'Log.entryAdded' && message.params?.entry?.level === 'error') diagnostics.logErrors.push(message.params.entry);
});

function call(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    const timeout = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`CDP ${method} timed out`));
    }, 30_000);
    pending.set(id, {
      resolve: (value) => { clearTimeout(timeout); resolve(value); },
      reject: (error) => { clearTimeout(timeout); reject(error); },
    });
    socket.send(JSON.stringify({ id, method, params }));
  });
}

async function evaluate(expression) {
  const response = await call('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (response.exceptionDetails) throw new Error(response.exceptionDetails.exception?.description || response.exceptionDetails.text);
  return response.result.value;
}

async function invokeResult(command, args = {}) {
  return evaluate(`(async()=>{
    try { return {ok:true,value:await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})}; }
    catch(error) { return {ok:false,error:{display:String(error),code:error?.code,messageKey:error?.messageKey,retryable:error?.retryable,debugId:error?.debugId}}; }
  })()`);
}

async function invoke(command, args = {}) {
  const result = await invokeResult(command, args);
  if (!result.ok) throw new Error(`${command} failed: ${JSON.stringify(result.error)}`);
  return result.value;
}

async function waitFor(expression, description, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if (await evaluate(expression)) return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  const state = await evaluate(`({route:location.pathname,lang:document.documentElement.lang,text:document.body?.innerText.slice(0,7000)})`);
  throw new Error(`Timed out waiting for ${description}: ${JSON.stringify(state)}`);
}

async function screenshot(filePath) {
  const result = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  await fs.writeFile(filePath, Buffer.from(result.data, 'base64'));
}

async function startAndCancel(meetingId, historicalGenerationId = null) {
  const started = await invoke('api_process_transcript', {
    text: 'The team reviewed progress, made a decision, and assigned an action item.',
    model: 'ollama',
    modelName: 'meetily-runtime-audit-model',
    meetingId,
    chunkSize: 40000,
    overlap: 1000,
    customPrompt: '',
    templateId: 'standard_meeting',
    historicalGenerationId,
    summaryLanguage: 'zh-CN',
  });
  const cancelled = await invoke('api_cancel_summary', { meetingId });
  return { started, cancelled };
}

await call('Runtime.enable');
await call('Log.enable');
await call('Page.enable');
await call('Emulation.setDeviceMetricsOverride', { width: 1600, height: 1000, deviceScaleFactor: 1, mobile: false });
await waitFor("Boolean(window.__TAURI_INTERNALS__?.invoke && document.readyState === 'complete')", 'Tauri IPC readiness', 60_000);
const runtimeOrigin = await evaluate(`({href:location.href,origin:location.origin,protocol:location.protocol})`);
console.log(JSON.stringify({ runtimeOrigin }));

let report;
try {
  if (await invoke('check_first_launch')) await invoke('initialize_fresh_database');
  await invoke('save_onboarding_status_cmd', {
    status: {
      version: '1.0', completed: true, current_step: 4,
      model_status: { parakeet: 'not_downloaded', summary: 'not_downloaded' },
      last_updated: new Date().toISOString(),
    },
  });
  await evaluate(`(()=>{localStorage.setItem('meetily.uiLocale','zh-CN');window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:'zh-CN'}));})()`);

  const saved = await invoke('api_save_transcript', {
    meetingTitle: `P2-300 lifecycle audit ${runId}`,
    transcripts: [{
      id: `${runId}_segment`, text: 'The team reviewed progress and assigned an action item.',
      timestamp: new Date().toISOString(), audio_start_time: 0, audio_end_time: 2, duration: 2,
    }],
    folderPath: workDirectory,
  });
  const meetingId = saved.meeting_id;
  const first = await startAndCancel(meetingId);
  const second = await startAndCancel(meetingId);
  const third = await startAndCancel(meetingId);
  const initialHistory = await invoke('api_list_summary_generation_history', { meetingId });
  const exactDetails = await invoke('api_get_summary_generation_snapshot', {
    request: { meetingId, generationId: first.started.generationId },
  });
  const replay = await startAndCancel(meetingId, first.started.generationId);
  const replayHistory = await invoke('api_list_summary_generation_history', { meetingId });

  const snapshotDirectory = path.join(workDirectory, 'summary-template-snapshots');
  const directOrphanId = 'gen_runtime_corrupt_orphan';
  const directOrphanPath = path.join(snapshotDirectory, `${directOrphanId}.json`);
  await fs.writeFile(directOrphanPath, '{corrupt audit snapshot', 'utf8');
  const policy = { retainLatest: 0, retainDays: 1, maxTotalBytes: 1 };
  const cleanupPreview = await invoke('api_preview_template_snapshot_cleanup', {
    request: { meetingId, policy },
  });
  const cleanupResult = await invoke('api_execute_template_snapshot_cleanup', {
    request: {
      meetingId,
      policy,
      previewToken: cleanupPreview.previewToken,
      expectedGenerationIds: cleanupPreview.items.filter((item) => item.cleanupCandidate).map((item) => item.generationId),
    },
  });
  const afterDirectPreview = await invoke('api_preview_template_snapshot_cleanup', {
    request: { meetingId, policy },
  });

  const uiOrphanId = 'gen_runtime_ui_corrupt_orphan';
  const uiOrphanPath = path.join(snapshotDirectory, `${uiOrphanId}.json`);
  await fs.writeFile(uiOrphanPath, '{corrupt UI audit snapshot', 'utf8');
  await call('Page.navigate', { url: `http://tauri.localhost/meeting-details?id=${encodeURIComponent(meetingId)}` });
  await waitFor("location.pathname==='/meeting-details' && document.documentElement.lang==='zh-CN' && document.body.innerText.includes('历史')", 'Chinese meeting details');
  await evaluate(`(()=>{const button=[...document.querySelectorAll('button')].find((node)=>(node.textContent||'').trim()==='历史');if(!button)throw new Error('History button missing');button.click();return true;})()`);
  await waitFor("document.body.innerText.includes('摘要生成历史') && document.body.innerText.includes('生成 ID')", 'generation history dialog');
  await evaluate(`(()=>{const button=[...document.querySelectorAll('button')].find((node)=>(node.textContent||'').includes('查看快照'));if(!button)throw new Error('Snapshot details button missing');button.click();return true;})()`);
  await waitFor("document.body.innerText.includes('语义 SHA-256') && document.body.innerText.includes('源文件 SHA-256')", 'immutable snapshot details');
  await screenshot(historyScreenshotPath);
  await evaluate(`(()=>{const button=[...document.querySelectorAll('button')].find((node)=>(node.textContent||'').includes('预览清理'));if(!button)throw new Error('Cleanup preview button missing');button.click();return true;})()`);
  await waitFor("document.body.innerText.includes('候选 1 个') && document.body.innerText.includes('隔离 1 个文件')", 'cleanup dry-run UI');
  await evaluate(`(()=>{const dialog=document.querySelector('[role=dialog]');if(dialog)dialog.scrollTop=dialog.scrollHeight;return true;})()`);
  await screenshot(cleanupScreenshotPath);
  const uiBeforeExecute = await evaluate(`({
    htmlLang:document.documentElement.lang,
    text:document.body.innerText,
    hasRawCode:/MEETING_TEMPLATE_|TEMPLATE_IO_ERROR|\\\\AppData\\\\|[A-Z]:\\\\Users\\\\/i.test(document.body.innerText)
  })`);
  await evaluate(`(()=>{const button=[...document.querySelectorAll('button')].find((node)=>(node.textContent||'').includes('隔离 1 个文件'));if(!button||button.disabled)throw new Error('Cleanup execute button missing');button.click();return true;})()`);
  await waitFor("!document.body.innerText.includes('隔离 1 个文件')", 'cleanup UI completion');
  const finalPreview = await invoke('api_preview_template_snapshot_cleanup', {
    request: { meetingId, policy },
  });
  const finalHistory = await invoke('api_list_summary_generation_history', { meetingId });
  const testMeetingDeletion = await invoke('api_delete_meeting', { meetingId, authToken: null });
  const deletedMeetingLookup = await invokeResult('api_get_meeting', { meetingId, authToken: null });
  const priorAuditCleanup = [];
  for (const priorMeetingId of priorAuditMeetingIds) {
    priorAuditCleanup.push({
      meetingId: priorMeetingId,
      result: await invokeResult('api_delete_meeting', { meetingId: priorMeetingId, authToken: null }),
    });
  }

  const assertions = {
    durableHistoryCreated: initialHistory.length === 3 && initialHistory.every((item) => item.generationId && item.status === 'cancelled'),
    immutableSnapshotDetailsReadable: exactDetails.generationId === first.started.generationId
      && exactDetails.semanticSha256?.length === 64 && exactDetails.template?.sections?.length > 0,
    exactHistoricalReplayUsed: replay.started.resolvedTemplate?.resolutionSource === 'historical_snapshot'
      && replayHistory.some((item) => item.generationId === replay.started.generationId),
    supersededAndCancelledIdsStable: [first, second, third, replay].every((entry) => entry.started.generationId === entry.cancelled.generation_id),
    directDryRunFindsOnlyCorruptOrphan: cleanupPreview.candidateFileCount === 1
      && cleanupPreview.items.find((item) => item.generationId === directOrphanId)?.cleanupCandidate === true,
    validSnapshotsProtected: cleanupPreview.protectedFileCount >= 4
      && cleanupPreview.items.filter((item) => item.fileState === 'available').every((item) => !item.cleanupCandidate),
    directCleanupMatchesPreview: cleanupResult.quarantinedFileCount === 1
      && cleanupResult.quarantinedGenerationIds?.[0] === directOrphanId
      && cleanupResult.planChanged === false && cleanupResult.skippedGenerationIds.length === 0,
    directSourceRemovedAfterQuarantine: !(await fs.stat(directOrphanPath).then(() => true).catch(() => false)),
    dryRunAfterDirectCleanupIsEmpty: afterDirectPreview.candidateFileCount === 0,
    chineseHistoryAndSnapshotVisible: uiBeforeExecute.htmlLang === 'zh-CN'
      && uiBeforeExecute.text.includes('摘要生成历史') && uiBeforeExecute.text.includes('语义 SHA-256'),
    uiDryRunVisible: uiBeforeExecute.text.includes('候选 1 个') && uiBeforeExecute.text.includes('隔离 1 个文件'),
    uiCleanupQuarantinedOrphan: !(await fs.stat(uiOrphanPath).then(() => true).catch(() => false))
      && finalPreview.candidateFileCount === 0,
    historyUnaffectedByCleanup: finalHistory.length === replayHistory.length
      && finalHistory.every((item) => item.snapshotState === 'available'),
    noRawErrorOrPathBubbling: !uiBeforeExecute.hasRawCode,
    zeroFrontendDiagnostics: diagnostics.exceptions.length === 0 && diagnostics.consoleErrors.length === 0 && diagnostics.logErrors.length === 0,
    testMeetingRemovedFromDatabase: testMeetingDeletion?.status === 'success' && deletedMeetingLookup.ok === false,
    priorAuditMeetingsRemoved: priorAuditCleanup.every((item) => item.result.ok && item.result.value?.status === 'success'),
  };
  report = {
    auditId: runId,
    timestamp: new Date().toISOString(),
    candidate: path.join(root, 'target-phase5-release', 'release', 'meetily.exe'),
    meetingId,
    generations: { first, second, third, replay, initialHistory, replayHistory, finalHistory },
    snapshotDetails: exactDetails,
    cleanup: { policy, preview: cleanupPreview, result: cleanupResult, afterDirectPreview, finalPreview },
    testDataCleanup: { databaseMeetingDeleted: true, priorAuditCleanup, workDirectory },
    ui: { historyScreenshotPath, cleanupScreenshotPath, htmlLang: uiBeforeExecute.htmlLang, runtimeOrigin },
    diagnostics: {
      exceptions: diagnostics.exceptions.length,
      consoleErrors: diagnostics.consoleErrors.length,
      logErrors: diagnostics.logErrors.length,
    },
    assertions,
    passed: Object.values(assertions).every(Boolean),
  };
  await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
  if (!report.passed) throw new Error(`P2-300 runtime assertions failed: ${JSON.stringify(assertions)}`);
} finally {
  socket.close();
}

console.log(JSON.stringify({ reportPath, historyScreenshotPath, cleanupScreenshotPath, passed: report?.passed, assertions: report?.assertions }, null, 2));
