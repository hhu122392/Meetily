#!/usr/bin/env node

import fs from 'node:fs/promises';
import path from 'node:path';

const port = Number(process.argv[2] || 9368);
const root = path.resolve(process.argv[3] || '.');
const runId = `runtime_import_editor_${Date.now()}`;
const workDirectory = path.join(root, 'target-phase5-release', 'runtime-template-import-editor', runId);
const evidenceDirectory = path.join(
  root,
  'target',
  'release',
  'docs',
  'phase-0-custom-summary-templates',
  'audit',
);
const reportPath = path.join(evidenceDirectory, 'template-import-editor-runtime-audit.json');
await fs.mkdir(workDirectory, { recursive: true });
await fs.mkdir(evidenceDirectory, { recursive: true });

const timestamp = new Date().toISOString();
const originalIds = [`${runId}_one`, `${runId}_two`];
const conflictId = `${runId}_conflict`;
const editedSecondId = `${runId}_edited_two`;
const lowConfidenceDocxPath = path.join(workDirectory, 'low-confidence.docx');
const lowConfidenceDocxBase64 = 'UEsDBBQAAAAIAFKsF13dEsBqbgAAAHkAAAATAAAAW0NvbnRlbnRfVHlwZXNdLnhtbDzNSQrDMAxA0auUbEvtCzjZ9ADd9ALGlovAGpCVobcPJdDtf4uf3l+FsaTXBmZY4fYUdmD/5XnKqh1LdhSOG9cgCnxQb2KUfTykNSxQpawE7GEXq2pSYAzkD/XwF8rI94P6FJcUr+MJAAD//wMAUEsDBBQAAAAIAFKsF10mcQC6fwAAAKwAAAARAAAAd29yZC9kb2N1bWVudC54bWyyKbdKyU8uzU3NK1GoyM3JK7Yqt1UqLcqzKkktLlGysym3SspPqQTRBSCiCESU2D2bue5lw6wne2a9WLfuxboNT/dOtdEHiYPIIjAJVo2s5cW6FS/W7Xqyq+3FuoVPOyY9ndTzcnHfy5krHzc0YdGrD7NXH+E+OwAAAAD//wMAUEsBAhQAFAAAAAgAUqwXXd0SwGpuAAAAeQAAABMAAAAAAAAAAAAAAAAAAAAAAFtDb250ZW50X1R5cGVzXS54bWxQSwECFAAUAAAACABSrBddJnEAun8AAACsAAAAEQAAAAAAAAAAAAAAAACfAAAAd29yZC9kb2N1bWVudC54bWxQSwUGAAAAAAIAAgCAAAAATQEAAAAA';
await fs.writeFile(lowConfidenceDocxPath, Buffer.from(lowConfidenceDocxBase64, 'base64'));
const inputPaths = [];
for (let index = 0; index < 2; index += 1) {
  const filePath = path.join(workDirectory, `draft-${index + 1}.json`);
  inputPaths.push(filePath);
  await fs.writeFile(filePath, `${JSON.stringify({
    schema_version: 2,
    id: originalIds[index],
    name: `导入原稿 ${index + 1}`,
    description: `第 ${index + 1} 个独立导入草稿`,
    version: 1,
    locale: 'zh-CN',
    tags: ['运行时审计'],
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
      title: '原始总结',
      instruction: `提取第 ${index + 1} 个原始结论。`,
      format: 'paragraph',
      item_format: null,
      example_item_format: null,
      required: true,
      empty_behavior: 'show_not_mentioned',
    }],
    extensions: { runtimeImportEditorAudit: runId, queueIndex: index + 1 },
  }, null, 2)}\n`, 'utf8');
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
  if (message.method === 'Runtime.consoleAPICalled' && message.params?.type === 'error') {
    diagnostics.consoleErrors.push(message.params);
  }
  if (message.method === 'Log.entryAdded' && message.params?.entry?.level === 'error') {
    diagnostics.logErrors.push(message.params.entry);
  }
});

function call(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    const timeout = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`CDP ${method} timed out`));
    }, 15_000);
    pending.set(id, {
      resolve: (value) => {
        clearTimeout(timeout);
        resolve(value);
      },
      reject: (error) => {
        clearTimeout(timeout);
        reject(error);
      },
    });
    socket.send(JSON.stringify({ id, method, params }));
  });
}

function progress(step) {
  process.stderr.write(`[runtime-import-editor] ${step}\n`);
}

async function evaluate(expression) {
  const response = await call('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (response.exceptionDetails) {
    throw new Error(response.exceptionDetails.exception?.description || response.exceptionDetails.text);
  }
  return response.result.value;
}

async function invoke(command, args = {}) {
  const result = await evaluate(`(async()=>{
    try {
      return {ok:true,value:await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})};
    } catch(error) {
      return {ok:false,error:{
        code:error?.code,
        messageKey:error?.messageKey,
        params:error?.params,
        fieldErrors:error?.fieldErrors,
        retryable:error?.retryable,
        debugId:error?.debugId,
      }};
    }
  })()`);
  if (!result.ok) {
    const error = new Error(`${command} failed: ${JSON.stringify(result.error)}`);
    Object.assign(error, result.error);
    throw error;
  }
  return result.value;
}

async function waitFor(expression, description, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if (await evaluate(expression)) return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  const state = await evaluate(`({route:location.pathname,text:document.body?.innerText.slice(0,2500)})`);
  throw new Error(`Timed out waiting for ${description}: ${JSON.stringify(state)}`);
}

async function clickButton(text) {
  const clicked = await evaluate(`(()=>{
    const text=${JSON.stringify(text)};
    const button=[...document.querySelectorAll('button')].find((element)=>(element.textContent||'').trim().includes(text));
    if(!button||button.disabled)return false;
    button.click();
    return true;
  })()`);
  if (!clicked) throw new Error(`Could not click button containing ${text}`);
}

async function setLabelControl(labelText, value, tagName = 'input') {
  const changed = await evaluate(`(()=>{
    const label=[...document.querySelectorAll('label')].find((element)=>(element.textContent||'').includes(${JSON.stringify(labelText)}));
    const control=label?.querySelector(${JSON.stringify(tagName)});
    if(!control)return false;
    const prototype=control instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype,'value').set.call(control,${JSON.stringify(value)});
    control.dispatchEvent(new Event('input',{bubbles:true}));
    control.dispatchEvent(new Event('change',{bubbles:true}));
    return true;
  })()`);
  if (!changed) throw new Error(`Could not set ${labelText}`);
}

async function ensureFirstSectionExpanded() {
  const alreadyExpanded = await evaluate(`Boolean(
    [...document.querySelectorAll('label')].find((element)=>(element.textContent||'').includes('信息提取指令'))?.querySelector('textarea')
  )`);
  if (alreadyExpanded) return;
  const expanded = await evaluate(`(()=>{
    const button=[...document.querySelectorAll('button')].find((element)=>(element.getAttribute('aria-label')||'').startsWith('展开'));
    if(!button)return false;
    button.click();
    return true;
  })()`);
  if (!expanded) throw new Error('Could not expand the first imported section');
  await waitFor(
    "Boolean([...document.querySelectorAll('label')].find((element)=>(element.textContent||'').includes('信息提取指令'))?.querySelector('textarea'))",
    'first section editor expansion',
  );
}

await call('Runtime.enable');
await call('Log.enable');
await call('Page.enable');
await call('Emulation.setDeviceMetricsOverride', {
  width: 1600,
  height: 1000,
  deviceScaleFactor: 1,
  mobile: false,
});
await waitFor(
  "Boolean(window.__TAURI_INTERNALS__?.invoke && document.readyState === 'complete')",
  'Tauri IPC readiness',
  45_000,
);

let initialOnboarding;
let existingDetails;
let savedDetails;
let report;
let audioDropFeedbackObserved = false;
let draftScreenshotPath = null;
let lowConfidenceScreenshotPath = null;
let lowConfidenceBlockedBeforeConfirmation = false;
let lowConfidenceEnabledAfterConfirmation = false;
const trashIds = [];
try {
  progress('read onboarding');
  initialOnboarding = await invoke('get_onboarding_status');
  await invoke('save_onboarding_status_cmd', {
    status: {
      version: '1.0',
      completed: true,
      current_step: 4,
      model_status: { parakeet: 'not_downloaded', summary: 'not_downloaded' },
      last_updated: new Date().toISOString(),
    },
  });

  const staleAuditTemplates = await invoke('api_list_templates_v2', {
    request: { origin: 'custom', includeInvalid: true, contentLocale: 'zh-CN' },
  });
  for (const template of staleAuditTemplates.templates.filter((item) => item.id.startsWith('runtime_import_editor_'))) {
    try {
      const deleted = await invoke('api_delete_template', {
        request: { templateId: template.id, expectedFileSha256: template.fileSha256 },
      });
      await invoke('api_purge_template', { request: { trashId: deleted.trashId } });
    } catch {}
  }

  existingDetails = await invoke('api_create_template', {
    request: {
      template: {
        schemaVersion: 2,
        id: conflictId,
        name: '既有冲突模板',
        description: '验证冲突检测使用编辑后的模板 ID。',
        version: 1,
        locale: 'zh-CN',
        tags: ['运行时审计'],
        source: {
          type: 'manual',
          originalFileName: null,
          originalFileSha256: null,
          importedAt: null,
          copiedFromTemplateId: null,
        },
        createdAt: timestamp,
        updatedAt: timestamp,
        sections: [{
          id: 'summary',
          title: '既有总结',
          instruction: '保留既有模板。',
          format: 'paragraph',
          itemFormat: null,
          exampleItemFormat: null,
          required: true,
          emptyBehavior: 'show_not_mentioned',
        }],
        extensions: { runtimeImportEditorAudit: runId, existingConflict: true },
      },
      conflictPolicy: 'error',
    },
  });

  progress('prepare Chinese template library');
  await call('Page.reload');
  await waitFor(
    "Boolean(window.__TAURI_INTERNALS__?.invoke && document.readyState === 'complete')",
    'IPC after onboarding reload',
    30_000,
  );
  await evaluate(`(()=>{
    localStorage.setItem('meetily.uiLocale','zh-CN');
    window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:'zh-CN'}));
  })()`);
  await call('Page.navigate', { url: 'http://tauri.localhost/settings/templates' });
  await waitFor(
    "location.pathname==='/settings/templates' && document.documentElement?.lang==='zh-CN' && document.body?.innerText.includes('导入模板')",
    'Chinese template library',
  );
  await new Promise((resolve) => setTimeout(resolve, 1_500));

  await invoke('plugin:event|emit_to', {
    target: { kind: 'Webview', label: 'main' },
    event: 'tauri://drag-drop',
    payload: { paths: inputPaths, position: { x: 600, y: 350 } },
  });
  progress('drag-drop emitted');
  await waitFor(
    `document.body.innerText.includes('draft-1.json') && document.body.innerText.includes('draft-2.json') && document.body.innerText.includes('编辑草稿')`,
    'two-item import review queue',
    30_000,
  );
  audioDropFeedbackObserved = await evaluate(`
    document.body.innerText.includes('请拖放音频文件') ||
    document.body.innerText.includes('拖放音频文件以导入')
  `);

  await clickButton('编辑草稿');
  progress('edit first draft');
  await setLabelControl('模板名称', '第一项临时编辑');
  await setLabelControl('模板 ID', conflictId);
  await ensureFirstSectionExpanded();
  await setLabelControl('信息提取指令', '编辑后的第一项提取规则。', 'textarea');
  await waitFor("document.body.innerText.includes('草稿校验通过')", 'first draft validation');

  await clickButton('draft-2.json');
  progress('switch to second draft');
  await waitFor(
    "[...document.querySelectorAll('label')].some((label)=>(label.textContent||'').includes('模板名称') && label.querySelector('input')?.value==='导入原稿 2')",
    'second draft independent baseline',
  );
  await setLabelControl('模板名称', '第二项保存前已编辑');
  await setLabelControl('模板 ID', editedSecondId);
  await ensureFirstSectionExpanded();
  await setLabelControl('信息提取指令', '保存编辑后的第二项规则。', 'textarea');
  await waitFor("document.body.innerText.includes('草稿校验通过')", 'second draft validation');

  await clickButton('draft-1.json');
  progress('switch back and restore first draft');
  await waitFor(
    `[...document.querySelectorAll('label')].some((label)=>(label.textContent||'').includes('模板 ID') && label.querySelector('input')?.value===${JSON.stringify(conflictId)})`,
    'first draft retained after queue switching',
  );
  await clickButton('恢复导入结果');
  await waitFor(
    `[...document.querySelectorAll('label')].some((label)=>(label.textContent||'').includes('模板 ID') && label.querySelector('input')?.value===${JSON.stringify(originalIds[0])})`,
    'restored imported baseline',
  );
  await setLabelControl('模板名称', '第一项冲突草稿');
  await setLabelControl('模板 ID', conflictId);
  await waitFor("document.body.innerText.includes('草稿校验通过')", 'restored then re-edited first draft validation');

  const policyChanged = await evaluate(`(()=>{
    const select=[...document.querySelectorAll('select')].find((element)=>element.getAttribute('aria-label')==='模板 ID 已存在时');
    if(!select)return false;
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(select,'skip');
    select.dispatchEvent(new Event('change',{bubbles:true}));
    return select.value==='skip';
  })()`);
  if (!policyChanged) throw new Error('Could not select skip-existing conflict policy');

  progress('save edited drafts');
  await waitFor(
    "[...document.querySelectorAll('button')].some((button)=>(button.textContent||'').includes('保存可用模板（2）') && !button.disabled)",
    'both edited drafts saveable',
  );
  const draftScreenshot = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  draftScreenshotPath = path.join(evidenceDirectory, 'template-import-editor-draft-zh-CN.png');
  await fs.writeFile(draftScreenshotPath, Buffer.from(draftScreenshot.data, 'base64'));
  await clickButton('保存可用模板');
  await waitFor(
    "document.body.innerText.includes('因 ID 已存在而跳过') && document.body.innerText.includes('已保存')",
    'edited-ID conflict skip and second draft save',
    30_000,
  );

  const listed = await invoke('api_list_templates_v2', {
    request: { origin: 'custom', includeInvalid: true, contentLocale: 'zh-CN' },
  });
  progress('verify persistence and capture evidence');
  savedDetails = listed.templates.find((template) => template.id === editedSecondId);
  const savedFull = savedDetails
    ? await invoke('api_get_template_v2', { request: { templateId: editedSecondId, origin: 'custom' } })
    : null;
  const originalOneWasCreated = listed.templates.some((template) => template.id === originalIds[0]);
  const originalTwoWasCreated = listed.templates.some((template) => template.id === originalIds[1]);
  const existingAfter = await invoke('api_get_template_v2', {
    request: { templateId: conflictId, origin: 'custom' },
  });

  const uiState = await evaluate(`(()=>({
    route:location.pathname,
    locale:document.documentElement.lang,
    hasEditedBadge:document.body.innerText.includes('已编辑'),
    hasRestoreAction:document.body.innerText.includes('恢复导入结果'),
    exposedKeys:(document.body.innerText.match(/templates:[A-Za-z0-9_.-]+/g)||[]),
    replacementCharacters:(document.body.innerText.match(/�/g)||[]).length,
  }))()`);
  const screenshot = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  const screenshotPath = path.join(evidenceDirectory, 'template-import-editor-runtime-zh-CN.png');
  await fs.writeFile(screenshotPath, Buffer.from(screenshot.data, 'base64'));

  await clickButton('关闭');
  await waitFor("!document.body.innerText.includes('draft-1.json')", 'completed JSON import dialog close');
  await new Promise((resolve) => setTimeout(resolve, 500));
  await invoke('plugin:event|emit_to', {
    target: { kind: 'Webview', label: 'main' },
    event: 'tauri://drag-drop',
    payload: { paths: [lowConfidenceDocxPath], position: { x: 600, y: 350 } },
  });
  await waitFor(
    "document.body.innerText.includes('low-confidence.docx') && document.body.innerText.includes('低置信度转换必须人工确认')",
    'low-confidence Word review gate',
    30_000,
  );
  lowConfidenceBlockedBeforeConfirmation = await evaluate(`
    [...document.querySelectorAll('button')].some((button)=>(button.textContent||'').includes('保存可用模板（0）') && button.disabled)
  `);
  const confirmationChecked = await evaluate(`(()=>{
    const label=[...document.querySelectorAll('label')].find((element)=>(element.textContent||'').includes('我已检查此草稿'));
    const checkbox=label?.querySelector('input[type="checkbox"]');
    if(!checkbox)return false;
    checkbox.click();
    return checkbox.checked;
  })()`);
  if (!confirmationChecked) throw new Error('Could not confirm the low-confidence Word draft');
  await waitFor(
    "[...document.querySelectorAll('button')].some((button)=>(button.textContent||'').includes('保存可用模板（1）') && !button.disabled)",
    'low-confidence save enabled after explicit confirmation',
  );
  lowConfidenceEnabledAfterConfirmation = true;
  const lowConfidenceScreenshot = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  lowConfidenceScreenshotPath = path.join(evidenceDirectory, 'template-import-low-confidence-zh-CN.png');
  await fs.writeFile(lowConfidenceScreenshotPath, Buffer.from(lowConfidenceScreenshot.data, 'base64'));
  await clickButton('关闭');
  await waitFor("document.body.innerText.includes('放弃未完成的导入？')", 'unfinished low-confidence queue confirmation');
  await clickButton('放弃队列');
  await waitFor("!document.body.innerText.includes('low-confidence.docx')", 'low-confidence queue discarded without saving');

  const assertions = {
    twoJsonFilesEnteredSharedDragQueue: true,
    templateDropDidNotReachAudioFeedback: !audioDropFeedbackObserved,
    lowConfidenceBlockedBeforeConfirmation,
    lowConfidenceEnabledAfterConfirmation,
    firstDraftEditRetainedAcrossQueueSwitch: true,
    secondDraftStartedFromIndependentBaseline: true,
    restoreReturnedFirstDraftToImportedBaseline: true,
    bothEditedDraftsPassedNativeValidation: true,
    editedIdDroveConflictPolicy:
      existingAfter.template.name === '既有冲突模板' && !originalOneWasCreated,
    editedSecondDraftPersisted:
      savedFull?.template?.id === editedSecondId &&
      savedFull?.template?.name === '第二项保存前已编辑' &&
      savedFull?.template?.sections?.[0]?.instruction === '保存编辑后的第二项规则。',
    originalIdsWereNotPersisted: !originalOneWasCreated && !originalTwoWasCreated,
    chineseEditorUiClean:
      uiState.route === '/settings/templates' &&
      uiState.locale === 'zh-CN' &&
      uiState.exposedKeys.length === 0 &&
      uiState.replacementCharacters === 0,
  };
  report = {
    audit: 'template-import-draft-editor-runtime',
    generatedAt: new Date().toISOString(),
    candidate: await evaluate('location.href'),
    runId,
    passed: Object.values(assertions).every(Boolean),
    assertions,
    results: {
      originalIds,
      conflictId,
      editedSecondId,
      savedName: savedFull?.template?.name ?? null,
      savedInstruction: savedFull?.template?.sections?.[0]?.instruction ?? null,
      audioDropFeedbackObserved,
      draftScreenshotPath,
      lowConfidenceScreenshotPath,
      uiState,
      screenshotPath,
    },
    diagnostics: {
      exceptions: diagnostics.exceptions.length,
      consoleErrors: diagnostics.consoleErrors.length,
      logErrors: diagnostics.logErrors.length,
    },
  };
} finally {
  for (const details of [savedDetails, existingDetails].filter(Boolean)) {
    try {
      const deleted = await invoke('api_delete_template', {
        request: { templateId: details.template?.id ?? details.id, expectedFileSha256: details.fileSha256 },
      });
      trashIds.push(deleted.trashId);
    } catch {}
  }
  for (const trashId of trashIds) {
    try {
      await invoke('api_purge_template', { request: { trashId } });
    } catch {}
  }
  try {
    if (initialOnboarding) await invoke('save_onboarding_status_cmd', { status: initialOnboarding });
    else if (initialOnboarding === null) await invoke('reset_onboarding_status_cmd');
  } catch {}
  socket.close();
}

if (!report) throw new Error('Runtime report was not produced');
const postCleanup = await fs.readFile(reportPath).catch(() => null);
report.cleanup = {
  purgedTrashEntries: trashIds.length,
  expectedTrashEntries: 2,
  complete: trashIds.length === 2,
  previousReportWasPresent: Boolean(postCleanup),
};
report.passed = report.passed && report.cleanup.complete &&
  report.diagnostics.exceptions === 0 &&
  report.diagnostics.consoleErrors === 0 &&
  report.diagnostics.logErrors === 0;
await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
process.stdout.write(`${JSON.stringify({ reportPath, ...report }, null, 2)}\n`);
if (!report.passed) process.exitCode = 1;
