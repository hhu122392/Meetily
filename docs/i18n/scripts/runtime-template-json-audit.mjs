#!/usr/bin/env node

import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';

const port = Number(process.argv[2] || 9367);
const root = path.resolve(process.argv[3] || '.');
const runId = `runtime_json_audit_${Date.now()}`;
const workingDirectory = path.join(root, 'target-phase5-release', 'runtime-template-json-audit', runId);
const evidenceDirectory = path.join(
  root,
  'target',
  'release',
  'docs',
  'phase-0-custom-summary-templates',
  'audit',
);
const reportPath = path.join(evidenceDirectory, 'template-json-runtime-audit.json');
const inputPath = path.join(workingDirectory, `${runId}.json`);
const invalidPath = path.join(workingDirectory, 'invalid.json');
const exportPath = path.join(workingDirectory, `${runId}.export.json`);
const batchPaths = [];
await fs.mkdir(workingDirectory, { recursive: true });
await fs.mkdir(evidenceDirectory, { recursive: true });

const timestamp = new Date().toISOString();
const input = {
  schema_version: 2,
  id: runId,
  name: '运行时 JSON 审计模板',
  description: 'Runtime JSON import/export audit — 中文保持完整',
  version: 7,
  locale: 'zh-CN',
  tags: ['audit', '中文'],
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
    id: 'decisions',
    title: '决策',
    instruction: '提取已经确认的决策。',
    format: 'list',
    item_format: '- {{decision}}',
    example_item_format: '- 发布中文版',
    required: true,
    empty_behavior: 'show_not_mentioned',
  }],
  extensions: { auditMarker: runId, nested: { preserved: true } },
};
await fs.writeFile(inputPath, `\uFEFF${JSON.stringify(input, null, 2)}\n`, 'utf8');
await fs.writeFile(invalidPath, '{\n  "schema_version": 2,\n  broken\n}\n', 'utf8');
for (let index = 0; index < 50; index += 1) {
  const batchInput = {
    ...input,
    id: `${runId}_batch_${String(index + 1).padStart(2, '0')}`,
    name: `运行时批量审计模板 ${index + 1}`,
    extensions: { auditMarker: runId, auditBatchIndex: index + 1 },
  };
  const batchPath = path.join(workingDirectory, `batch-${String(index + 1).padStart(2, '0')}.json`);
  batchPaths.push(batchPath);
  await fs.writeFile(batchPath, `${JSON.stringify(batchInput, null, 2)}\n`, 'utf8');
}

async function waitForTarget(timeoutMs = 45000) {
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
    pending.set(id, { resolve, reject });
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
        text:String(error),
      }};
    }
  })()`);
  if (!result.ok) {
    const detail = typeof result.error === 'string' ? result.error : JSON.stringify(result.error);
    const error = new Error(`${command} failed: ${detail}`);
    if (result.error && typeof result.error === 'object') Object.assign(error, result.error);
    error.cause = result.error;
    throw error;
  }
  return result.value;
}

await call('Runtime.enable');
await call('Log.enable');
await call('Page.enable');
{
  const deadline = Date.now() + 45000;
  let ready = false;
  while (Date.now() < deadline) {
    ready = await evaluate("Boolean(window.__TAURI_INTERNALS__?.invoke && document.readyState === 'complete')");
    if (ready) break;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  if (!ready) throw new Error('Tauri IPC did not become ready');
}

const created = [];
const trashIds = [];
let initialOnboarding;
let report;
let cleanupVerification = {
  residualTemplateIds: [],
  screenshotPath: null,
};
try {
  const previewResponse = await invoke('api_preview_template_imports', {
    request: { paths: [inputPath] },
  });
  const previewItem = previewResponse.items[0];
  if (!previewItem?.preview) throw new Error('Valid V2 JSON did not produce a preview');

  const invalidResponse = await invoke('api_preview_template_imports', {
    request: { paths: [invalidPath] },
  });
  const invalidError = invalidResponse.items[0]?.error;

  const batchPreviewResponse = await invoke('api_preview_template_imports', {
    request: { paths: batchPaths },
  });
  const batchCreated = [];
  for (const item of batchPreviewResponse.items) {
    if (!item.preview) continue;
    const details = await invoke('api_create_template', {
      request: { template: item.preview.draft, conflictPolicy: 'error' },
    });
    batchCreated.push(details);
    created.push(details);
  }

  const first = await invoke('api_create_template', {
    request: { template: previewItem.preview.draft, conflictPolicy: 'error' },
  });
  created.push(first);

  let duplicateError = null;
  try {
    await invoke('api_create_template', {
      request: { template: previewItem.preview.draft, conflictPolicy: 'error' },
    });
  } catch (error) {
    duplicateError = error;
  }

  const keptBoth = await invoke('api_create_template', {
    request: { template: previewItem.preview.draft, conflictPolicy: 'keep_both' },
  });
  created.push(keptBoth);

  const exportResponse = await invoke('api_export_template_json', {
    request: {
      templateId: first.template.id,
      origin: 'custom',
      contentLocale: 'zh-CN',
      destinationPath: exportPath,
    },
  });
  const exportedBytes = await fs.readFile(exportPath);
  const exported = JSON.parse(exportedBytes.toString('utf8'));
  const exportedSha256 = crypto.createHash('sha256').update(exportedBytes).digest('hex');
  const reimportResponse = await invoke('api_preview_template_imports', {
    request: { paths: [exportPath] },
  });
  const listed = await invoke('api_list_templates_v2', {
    request: { origin: 'custom', includeInvalid: true, contentLocale: 'zh-CN' },
  });

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
  await call('Page.reload');
  {
    const deadline = Date.now() + 30000;
    let ready = false;
    while (Date.now() < deadline) {
      ready = await evaluate("Boolean(window.__TAURI_INTERNALS__?.invoke && document.readyState === 'complete')");
      if (ready) break;
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
    if (!ready) throw new Error('Tauri IPC did not recover after onboarding-state reload');
  }
  await evaluate(`(()=>{
    localStorage.setItem('meetily.uiLocale','zh-CN');
    window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:'zh-CN'}));
  })()`);
  await call('Page.navigate', { url: 'http://tauri.localhost/settings/templates' });
  {
    const deadline = Date.now() + 10000;
    let ready = false;
    while (Date.now() < deadline) {
      ready = await evaluate(`Boolean(
        location.pathname.startsWith('/settings/templates') &&
        document.documentElement.lang==='zh-CN' &&
        document.body?.innerText.includes('导入模板')
      )`);
      if (ready) break;
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
    if (!ready) {
      const state = await evaluate(`({
        route:location.pathname,
        locale:document.documentElement.lang,
        text:document.body?.innerText.slice(0,500)
      })`);
      throw new Error(`Chinese template library did not become ready: ${JSON.stringify(state)}`);
    }
  }
  const menuButton = await evaluate(`(()=>{
    const button=[...document.querySelectorAll('button')].find((element)=>(element.getAttribute('aria-label')||'').includes('更多操作'));
    if(!button)return null;
    const rect=button.getBoundingClientRect();
    return {x:rect.left+rect.width/2,y:rect.top+rect.height/2};
  })()`);
  if (menuButton) {
    await call('Input.dispatchMouseEvent', {
      type: 'mousePressed', x: menuButton.x, y: menuButton.y, button: 'left', clickCount: 1,
    });
    await call('Input.dispatchMouseEvent', {
      type: 'mouseReleased', x: menuButton.x, y: menuButton.y, button: 'left', clickCount: 1,
    });
  }
  await new Promise((resolve) => setTimeout(resolve, 300));
  const uiAudit = await evaluate(`(()=>{
    const visible=(element)=>{const style=getComputedStyle(element);const rect=element.getBoundingClientRect();return style.display!=='none'&&style.visibility!=='hidden'&&rect.width>0&&rect.height>0};
    const controls=[...document.querySelectorAll('button,a,input,select,textarea,[role="button"],[role="menuitem"]')].filter(visible);
    const name=(element)=>element.getAttribute('aria-label')||element.getAttribute('title')||element.getAttribute('placeholder')||(element.textContent||'').trim();
    return {
      route:location.pathname,
      locale:document.documentElement.lang,
      hasImportEntry:document.body.innerText.includes('导入模板'),
      hasExportEntry:document.body.innerText.includes('导出 JSON'),
      unnamedControls:controls.filter((element)=>!name(element)).length,
      exposedTranslationKeys:(document.body.innerText.match(/templates:[A-Za-z0-9_.-]+/g)||[]),
      replacementCharacters:(document.body.innerText.match(/�/g)||[]).length,
    };
  })()`);
  const screenshot = await call('Page.captureScreenshot', {
    format: 'png',
    captureBeyondViewport: false,
  });
  const screenshotPath = path.join(evidenceDirectory, 'template-json-library-zh-CN.png');
  await fs.writeFile(screenshotPath, Buffer.from(screenshot.data, 'base64'));

  const assertions = {
    v2BomPreviewSucceeded:
      previewItem.preview.sourceType === 'json_import' &&
      previewItem.preview.draft.schemaVersion === 2,
    sourceMetadataNormalized:
      previewItem.preview.draft.source.type === 'json_import' &&
      previewItem.preview.draft.source.originalFileName === path.basename(inputPath) &&
      !previewItem.preview.draft.source.originalFileName.includes('\\'),
    invalidJsonHasLineAndColumn:
      invalidError?.code === 'TEMPLATE_JSON_INVALID' &&
      Number(invalidError.params?.line) === 3 &&
      Number(invalidError.params?.column) > 0,
    batchOfFiftyPreviewedSavedAndListed:
      batchPreviewResponse.items.length === 50 &&
      batchPreviewResponse.items.every((item) => item.preview?.draft?.schemaVersion === 2) &&
      batchCreated.length === 50 &&
      batchCreated.every((details) => listed.templates.some((template) => template.id === details.template.id)),
    persistedAndListed:
      listed.templates.some((template) => template.id === first.template.id && template.origin === 'custom'),
    duplicateRejected:
      duplicateError?.code === 'TEMPLATE_ALREADY_EXISTS' && typeof duplicateError.debugId === 'string',
    keepBothGeneratedUniqueId:
      keptBoth.template.id !== first.template.id && keptBoth.origin === 'custom',
    exportIsFormattedSnakeCaseUtf8:
      exported.schema_version === 2 &&
      exported.schemaVersion === undefined &&
      exported.name === input.name &&
      exported.sections[0].title === '决策' &&
      exportedBytes.includes(Buffer.from('\n  "schema_version"', 'utf8')),
    exportHashAndLengthMatch:
      exportResponse.fileSha256 === exportedSha256 && exportResponse.bytes === exportedBytes.length,
    exportContainsNoPrivatePath:
      !exportedBytes.toString('utf8').includes(workingDirectory) &&
      exported.source.original_file_name === path.basename(inputPath),
    exportedFileCanBeReimported:
      reimportResponse.items[0]?.preview?.draft?.id === first.template.id &&
      reimportResponse.items[0]?.preview?.draft?.extensions?.auditMarker === runId,
    chineseLibraryImportAndExportEntriesVisible:
      Boolean(menuButton) && uiAudit.hasImportEntry && uiAudit.hasExportEntry,
    chineseLibraryAccessibleAndLocalized:
      uiAudit.route === '/settings/templates' &&
      uiAudit.locale === 'zh-CN' &&
      uiAudit.unnamedControls === 0 &&
      uiAudit.exposedTranslationKeys.length === 0 &&
      uiAudit.replacementCharacters === 0,
  };
  report = {
    audit: 'template-json-import-export-runtime',
    generatedAt: new Date().toISOString(),
    candidate: target.url,
    runId,
    passed: Object.values(assertions).every(Boolean),
    assertions,
    results: {
      previewSourceType: previewItem.preview.sourceType,
      invalidError: invalidError ? {
        code: invalidError.code,
        line: invalidError.params?.line,
        column: invalidError.params?.column,
      } : null,
      createdTemplateId: first.template.id,
      keepBothTemplateId: keptBoth.template.id,
      batchPreviewed: batchPreviewResponse.items.length,
      batchSaved: batchCreated.length,
      exportedFileName: exportResponse.fileName,
      exportedBytes: exportResponse.bytes,
      exportedSha256,
      uiAudit,
      screenshotPath,
    },
    diagnostics: {
      exceptions: diagnostics.exceptions.length,
      consoleErrors: diagnostics.consoleErrors.length,
      logErrors: diagnostics.logErrors.length,
    },
  };
} finally {
  for (const details of created.reverse()) {
    try {
      const deleted = await invoke('api_delete_template', {
        request: {
          templateId: details.template.id,
          expectedFileSha256: details.fileSha256,
        },
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
    const postCleanupList = await invoke('api_list_templates_v2', {
      request: { origin: 'custom', includeInvalid: true, contentLocale: 'zh-CN' },
    });
    const createdIds = new Set(created.map((details) => details.template.id));
    cleanupVerification.residualTemplateIds = postCleanupList.templates
      .map((template) => template.id)
      .filter((templateId) => createdIds.has(templateId));
  } catch {}
  try {
    if (initialOnboarding) {
      await invoke('save_onboarding_status_cmd', { status: initialOnboarding });
    } else if (initialOnboarding === null) {
      await invoke('reset_onboarding_status_cmd');
    }
  } catch {}
  socket.close();
}

if (!report) throw new Error('Runtime report was not produced');
report.cleanup = {
  createdTemplates: created.length,
  purgedTrashEntries: trashIds.length,
  residualTemplateIds: cleanupVerification.residualTemplateIds,
  screenshotPath: cleanupVerification.screenshotPath,
  complete:
    trashIds.length === created.length &&
    cleanupVerification.residualTemplateIds.length === 0,
};
report.passed = report.passed && report.cleanup.complete &&
  report.diagnostics.exceptions === 0 &&
  report.diagnostics.consoleErrors === 0 &&
  report.diagnostics.logErrors === 0;
await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
process.stdout.write(`${JSON.stringify({ reportPath, ...report }, null, 2)}\n`);
if (!report.passed) process.exitCode = 1;
