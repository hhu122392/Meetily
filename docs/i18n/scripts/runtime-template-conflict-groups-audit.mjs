#!/usr/bin/env node

import fs from 'node:fs/promises';
import path from 'node:path';

const port = Number(process.argv[2] || 9368);
const root = path.resolve(process.argv[3] || '.');
const runId = `runtime_conflict_groups_${Date.now()}`;
const ids = {
  customA: `${runId}_custom_a`,
  customB: `${runId}_custom_b`,
  batch: `${runId}_batch`,
};
const workDirectory = path.join(root, 'target-phase5-release', 'runtime-template-conflicts', runId);
const evidenceDirectory = path.join(root, 'target', 'release', 'docs', 'phase-0-custom-summary-templates', 'audit');
const reportPath = path.join(evidenceDirectory, 'template-conflict-groups-runtime-audit.json');
const screenshotPath = path.join(evidenceDirectory, 'template-conflict-groups-zh-CN.png');
const prewriteScreenshotPath = path.join(evidenceDirectory, 'template-conflict-groups-prewrite-zh-CN.png');
await fs.mkdir(workDirectory, { recursive: true });
await fs.mkdir(evidenceDirectory, { recursive: true });

const timestamp = new Date().toISOString();
function template(id, name, instruction, marker) {
  return {
    schema_version: 2,
    id,
    name,
    description: `冲突分组运行时审计 ${marker}`,
    version: 1,
    locale: 'zh-CN',
    tags: ['冲突分组审计'],
    source: {
      type: 'manual',
      original_file_name: null,
      original_file_sha256: null,
      imported_at: null,
      copied_from_template_id: null,
    },
    created_at: timestamp,
    updated_at: timestamp,
    sections: [{
      id: 'summary',
      title: '会议总结',
      instruction,
      format: 'paragraph',
      item_format: null,
      example_item_format: null,
      required: true,
      empty_behavior: 'show_not_mentioned',
    }],
    extensions: { runtimeConflictGroupsAudit: runId, marker },
  };
}

const imports = [
  ['01-custom-a.json', template(ids.customA, '导入替换自定义 A', '写入自定义 A 的新规则。', 'custom-a')],
  ['02-custom-b.json', template(ids.customB, '导入替换自定义 B', '写入自定义 B 的新规则。', 'custom-b')],
  ['03-readonly.json', template('standard_meeting', '不应覆盖内置模板', '不得写入内置模板。', 'read-only')],
  ['04-batch-primary.json', template(ids.batch, '本批主模板', '保存本批第一个模板。', 'batch-primary')],
  ['05-batch-duplicate.json', template(ids.batch, '本批重复模板', '此重复项应跳过。', 'batch-duplicate')],
];
const inputPaths = [];
for (const [fileName, value] of imports) {
  const filePath = path.join(workDirectory, fileName);
  inputPaths.push(filePath);
  await fs.writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`, 'utf8');
}

async function waitForTarget(timeoutMs = 45_000) {
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
    }, 15_000);
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

async function invoke(command, args = {}) {
  const result = await evaluate(`(async()=>{
    try { return {ok:true,value:await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})}; }
    catch(error) { return {ok:false,error:{code:error?.code,messageKey:error?.messageKey,retryable:error?.retryable,debugId:error?.debugId}}; }
  })()`);
  if (!result.ok) throw new Error(`${command} failed: ${JSON.stringify(result.error)}`);
  return result.value;
}

async function waitFor(expression, description, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if (await evaluate(expression)) return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  const state = await evaluate(`({route:location.pathname,text:document.body?.innerText.slice(0,4000)})`);
  throw new Error(`Timed out waiting for ${description}: ${JSON.stringify(state)}`);
}

async function clickButton(text) {
  const clicked = await evaluate(`(()=>{
    const button=[...document.querySelectorAll('button')].find((element)=>(element.textContent||'').trim().includes(${JSON.stringify(text)}));
    if(!button||button.disabled)return false;
    button.click();
    return true;
  })()`);
  if (!clicked) throw new Error(`Could not click button containing ${text}`);
}

async function setDecision(value) {
  const changed = await evaluate(`(()=>{
    const select=[...document.querySelectorAll('select')].find((element)=>element.getAttribute('aria-label')==='此文件的冲突决策');
    if(!select)return false;
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(select,${JSON.stringify(value)});
    select.dispatchEvent(new Event('change',{bubbles:true}));
    return select.value===${JSON.stringify(value)};
  })()`);
  if (!changed) throw new Error(`Could not set conflict decision ${value}`);
}

async function currentDecision() {
  return evaluate(`(()=>[...document.querySelectorAll('select')].find((element)=>element.getAttribute('aria-label')==='此文件的冲突决策')?.value??null)()`);
}

async function createExisting(id, name) {
  return invoke('api_create_template', {
    request: {
      template: {
        schemaVersion: 2,
        id,
        name,
        description: '保存开始前必须保持不变。',
        version: 1,
        locale: 'zh-CN',
        tags: ['冲突分组审计'],
        source: { type: 'manual', originalFileName: null, originalFileSha256: null, importedAt: null, copiedFromTemplateId: null },
        createdAt: timestamp,
        updatedAt: timestamp,
        sections: [{
          id: 'summary', title: '既有总结', instruction: '保留既有规则。', format: 'paragraph', itemFormat: null,
          exampleItemFormat: null, required: true, emptyBehavior: 'show_not_mentioned',
        }],
        extensions: { runtimeConflictGroupsAudit: runId, existing: true },
      },
      conflictPolicy: 'error',
    },
  });
}

async function cleanupAuditTemplates() {
  const listed = await invoke('api_list_templates_v2', {
    request: { origin: 'custom', includeInvalid: true, includeTrash: false, contentLocale: 'zh-CN' },
  });
  let purged = 0;
  for (const item of listed.templates.filter((templateItem) => templateItem.id.startsWith('runtime_conflict_groups_'))) {
    try {
      const deleted = await invoke('api_delete_template', {
        request: { templateId: item.id, expectedFileSha256: item.fileSha256 },
      });
      await invoke('api_purge_template', { request: { trashId: deleted.trashId } });
      purged += 1;
    } catch {}
  }
  return purged;
}

await call('Runtime.enable');
await call('Log.enable');
await call('Page.enable');
await call('Emulation.setDeviceMetricsOverride', { width: 1600, height: 1000, deviceScaleFactor: 1, mobile: false });
await waitFor("Boolean(window.__TAURI_INTERNALS__?.invoke && document.readyState === 'complete')", 'Tauri IPC readiness', 45_000);

let initialOnboarding;
let report;
let cleanupCount = 0;
try {
  initialOnboarding = await invoke('get_onboarding_status');
  await invoke('save_onboarding_status_cmd', {
    status: {
      version: '1.0', completed: true, current_step: 4,
      model_status: { parakeet: 'not_downloaded', summary: 'not_downloaded' },
      last_updated: new Date().toISOString(),
    },
  });
  await cleanupAuditTemplates();
  await createExisting(ids.customA, '既有自定义 A');
  await createExisting(ids.customB, '既有自定义 B');

  await call('Page.reload');
  await waitFor("Boolean(window.__TAURI_INTERNALS__?.invoke && document.readyState === 'complete')", 'IPC after onboarding reload');
  await evaluate(`(()=>{localStorage.setItem('meetily.uiLocale','zh-CN');window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:'zh-CN'}));})()`);
  await call('Page.navigate', { url: 'http://tauri.localhost/settings/templates' });
  await waitFor("location.pathname==='/settings/templates' && document.documentElement.lang==='zh-CN' && document.body.innerText.includes('导入模板')", 'Chinese template library');
  await new Promise((resolve) => setTimeout(resolve, 1200));

  await invoke('plugin:event|emit_to', {
    target: { kind: 'Webview', label: 'main' },
    event: 'tauri://drag-drop',
    payload: { paths: inputPaths, position: { x: 600, y: 350 } },
  });
  await waitFor(
    "document.body.innerText.includes('01-custom-a.json') && document.body.innerText.includes('05-batch-duplicate.json') && document.body.innerText.includes('自定义：2') && document.body.innerText.includes('只读：1') && document.body.innerText.includes('本批重复：1')",
    'three conflict groups',
  );
  const initialUi = await evaluate(`(()=>({
    unresolved:document.body.innerText.includes('还有 4 项冲突未决策'),
    saveDisabled:[...document.querySelectorAll('button')].some((button)=>(button.textContent||'').includes('保存可用模板（1）')&&button.disabled),
    selectedDecision:[...document.querySelectorAll('select')].find((element)=>element.getAttribute('aria-label')==='此文件的冲突决策')?.value??null,
  }))()`);
  const beforeDecisionList = await invoke('api_list_templates_v2', {
    request: { origin: 'custom', includeInvalid: true, includeTrash: false, contentLocale: 'zh-CN' },
  });

  await setDecision('replace_custom');
  await clickButton('应用到同类型');
  await clickButton('02-custom-b.json');
  await waitFor("[...document.querySelectorAll('select')].some((select)=>select.getAttribute('aria-label')==='此文件的冲突决策'&&select.value==='replace_custom')", 'same-group custom decision');
  const customGroupApplied = await currentDecision() === 'replace_custom';

  await clickButton('03-readonly.json');
  const readOnlyWasIsolated = await currentDecision() === '';
  await setDecision('override_builtin');

  await clickButton('05-batch-duplicate.json');
  const batchWasIsolated = await currentDecision() === '';
  await setDecision('skip');
  await waitFor(
    "document.body.innerText.includes('检测到的冲突均已处理') && [...document.querySelectorAll('button')].some((button)=>(button.textContent||'').includes('保存可用模板（5）')&&!button.disabled)",
    'all per-item conflicts resolved',
  );

  await clickButton('保存可用模板');
  await waitFor("document.body.innerText.includes('确认覆盖内置模板') && document.body.innerText.includes('最多 1 个模板')", 'exact read-only override confirmation');
  const prewriteScreenshot = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  await fs.writeFile(prewriteScreenshotPath, Buffer.from(prewriteScreenshot.data, 'base64'));
  await clickButton('返回检查');
  await waitFor("!document.body.innerText.includes('确认覆盖内置模板')", 'override cancellation');

  const afterCancelList = await invoke('api_list_templates_v2', {
    request: { origin: 'custom', includeInvalid: true, includeTrash: false, contentLocale: 'zh-CN' },
  });
  const afterCancelCustomA = afterCancelList.templates.find((item) => item.id === ids.customA);
  const afterCancelCustomB = afterCancelList.templates.find((item) => item.id === ids.customB);
  const cancellationWroteNothing = afterCancelCustomA?.name === '既有自定义 A'
    && afterCancelCustomB?.name === '既有自定义 B'
    && !afterCancelList.templates.some((item) => item.id === ids.batch);

  await clickButton('03-readonly.json');
  await setDecision('skip');
  await waitFor("[...document.querySelectorAll('button')].some((button)=>(button.textContent||'').includes('保存可用模板（5）')&&!button.disabled)", 'save enabled after safe read-only decision');
  await clickButton('保存可用模板');
  await waitFor(
    "document.body.innerText.includes('已保存') && document.body.innerText.includes('因 ID 已存在而跳过')",
    'grouped conflict batch completion',
    45_000,
  );

  const finalList = await invoke('api_list_templates_v2', {
    request: { origin: 'custom', includeInvalid: true, includeTrash: false, contentLocale: 'zh-CN' },
  });
  const finalA = finalList.templates.find((item) => item.id === ids.customA);
  const finalB = finalList.templates.find((item) => item.id === ids.customB);
  const finalBatch = finalList.templates.find((item) => item.id === ids.batch);
  const standardCustomOverride = finalList.templates.find((item) => item.id === 'standard_meeting');
  const uiState = await evaluate(`(()=>({
    route:location.pathname,
    locale:document.documentElement.lang,
    exposedKeys:(document.body.innerText.match(/templates:[A-Za-z0-9_.-]+/g)||[]),
    replacementCharacters:(document.body.innerText.match(/�/g)||[]).length,
  }))()`);
  const screenshot = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  await fs.writeFile(screenshotPath, Buffer.from(screenshot.data, 'base64'));

  const assertions = {
    customReadOnlyAndBatchGroupsDetected: initialUi.unresolved,
    unresolvedConflictsBlockedWholeBatch: initialUi.saveDisabled,
    noImplicitDefaultDecision: initialUi.selectedDecision === '',
    preDecisionRepositoryUnchanged:
      beforeDecisionList.templates.find((item) => item.id === ids.customA)?.name === '既有自定义 A'
      && beforeDecisionList.templates.find((item) => item.id === ids.customB)?.name === '既有自定义 B'
      && !beforeDecisionList.templates.some((item) => item.id === ids.batch),
    sameTypeApplicationReachedBothCustomConflicts: customGroupApplied,
    sameTypeApplicationDidNotCrossIntoReadOnly: readOnlyWasIsolated,
    sameTypeApplicationDidNotCrossIntoBatch: batchWasIsolated,
    overrideConfirmationCountWasExact: true,
    cancellingOverrideConfirmationWroteNothing: cancellationWroteNothing,
    customReplacementsPersisted: finalA?.name === '导入替换自定义 A' && finalB?.name === '导入替换自定义 B',
    batchPrimaryPersistedAndDuplicateSkipped: finalBatch?.name === '本批主模板',
    readOnlyTemplateWasNotOverridden: !standardCustomOverride,
    chineseUiClean: uiState.route === '/settings/templates' && uiState.locale === 'zh-CN'
      && uiState.exposedKeys.length === 0 && uiState.replacementCharacters === 0,
  };
  report = {
    audit: 'template-import-conflict-groups-runtime',
    generatedAt: new Date().toISOString(),
    candidate: await evaluate('location.href'),
    runId,
    passed: Object.values(assertions).every(Boolean),
    assertions,
    results: {
      ids,
      detectedCounts: { custom: 2, readOnly: 1, batch: 1 },
      finalNames: { customA: finalA?.name ?? null, customB: finalB?.name ?? null, batch: finalBatch?.name ?? null },
      uiState,
      prewriteScreenshotPath,
      screenshotPath,
    },
    diagnostics: {
      exceptions: diagnostics.exceptions.length,
      consoleErrors: diagnostics.consoleErrors.length,
      logErrors: diagnostics.logErrors.length,
    },
  };
} finally {
  try { cleanupCount = await cleanupAuditTemplates(); } catch {}
  try {
    if (initialOnboarding) await invoke('save_onboarding_status_cmd', { status: initialOnboarding });
    else if (initialOnboarding === null) await invoke('reset_onboarding_status_cmd');
  } catch {}
  socket.close();
}

if (!report) throw new Error('Runtime conflict-group report was not produced');
report.cleanup = { purgedTemplates: cleanupCount, expectedTemplates: 3, complete: cleanupCount === 3 };
report.passed = report.passed && report.cleanup.complete
  && report.diagnostics.exceptions === 0
  && report.diagnostics.consoleErrors === 0
  && report.diagnostics.logErrors === 0;
await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
process.stdout.write(`${JSON.stringify({ reportPath, ...report }, null, 2)}\n`);
if (!report.passed) process.exitCode = 1;
