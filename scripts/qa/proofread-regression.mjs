#!/usr/bin/env node
/**
 * 「文字纠错 / AI 深查」回归台
 *
 * 为什么要这个东西：2026-09-12 那次故障里，本地 Qwen3.5-4B 漏掉了最明显的
 * 「年想赚到 → 你想赚到」，还自作聪明把「影片」改成「视频」。事后完全无法复盘，
 * 因为应用没有留下任何模型原始输出。这个脚本把同一份输入分别喂给本地模型 / API 模型，
 * 用固定用例量化「漏改」和「误改」，改提示词时先在这里跑通再上真机。
 *
 * 客户端和本脚本直接编译 proofread_protocol.rs，共用提示词、分批和解析。
 * 漏答重问一次；超过 60 段继续下一页。历史不完整标注只供诊断，不作通过依据。
 *
 * 用法：
 *   node scripts/qa/proofread-regression.mjs --model=local
 *   node scripts/qa/proofread-regression.mjs --model=api --api-key=sk-xxx
 *   node scripts/qa/proofread-regression.mjs --model=both --api-key=sk-xxx
 */
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '..', '..');
import { judge } from './proofread-score.mjs';

const fixturePath = join(here, 'fixtures', 'proofread-regression-cases.json');

const args = new Map(
  process.argv.slice(2).map((raw) => {
    const [key, ...rest] = raw.replace(/^--/, '').split('=');
    return [key, rest.join('=') || 'true'];
  }),
);
const which = args.get('model') ?? 'local';
const apiKey = args.get('api-key') ?? process.env.PROOFREAD_API_KEY ?? '';
const apiEndpoint = (args.get('api-endpoint') ?? 'https://api.deepseek.com').replace(/\/$/, '');
const apiModel = args.get('api-model') ?? 'deepseek-flash';
const helper =
  args.get('helper') ??
  join(repoRoot, 'target', 'release', 'llama-helper.exe');
const gguf =
  args.get('gguf') ??
  join(process.env.APPDATA ?? '', 'com.meetily.ai', 'models', 'summary', 'Qwen3.5-4B-Q4_K_M.gguf');
const outDir = args.get('out') ?? join(here, 'proofread-regression-out');

/** 文件名安全化：模型名/端点是外部输入，别把奇怪字符（乃至 key）带进文件名 */
const safeName = (value) => String(value).replace(/[^A-Za-z0-9._-]+/g, '_').slice(0, 40);

/** 本地内置模型的采样参数（models.rs::SamplingParams::qwen35_summary + DEFAULT_MAX_TOKENS） */
const LOCAL_SAMPLING = {
  max_tokens: 4096,
  context_size: 32768,
  temperature: 0.5,
  top_k: 20,
  top_p: 0.8,
  presence_penalty: 0.3,
  frequency_penalty: 0.0,
  repeat_penalty: 1.05,
  penalty_last_n: 256,
  stop_tokens: ['<|im_end|>'],
};

/** qwen3.5_nonthinking 模板（models.rs::QWEN35_NONTHINKING_TEMPLATE） */
const QWEN_TEMPLATE =
  '<|im_start|>system\n{system_prompt}<|im_end|>\n' +
  '<|im_start|>user\n{user_prompt}<|im_end|>\n' +
  '<|im_start|>assistant\n<think>\n\n</think>\n\n';

const bridgeDir = join(here, 'proofread-protocol');
const bridgeBin = join(bridgeDir, 'target', 'debug', process.platform === 'win32' ? 'meetily-proofread-protocol.exe' : 'meetily-proofread-protocol');
function protocol(request) {
  const result = spawnSync(bridgeBin, [], { input: JSON.stringify(request), encoding: 'utf8', maxBuffer: 32 * 1024 * 1024, windowsHide: true });
  if (result.status !== 0) throw new Error(result.stderr || 'protocol bridge failed');
  return JSON.parse(result.stdout);
}

function runLocal(systemPrompt, userPrompt) {
  const prompt = QWEN_TEMPLATE.replace('{system_prompt}', systemPrompt).replace('{user_prompt}', userPrompt);
  const request = JSON.stringify({
    type: 'generate',
    prompt,
    model_path: gguf,
    ...LOCAL_SAMPLING,
  });
  return new Promise((resolvePromise, reject) => {
    if (!existsSync(helper)) {
      reject(new Error(`llama-helper not found: ${helper}`));
      return;
    }
    if (!existsSync(gguf)) {
      reject(new Error(`gguf not found: ${gguf}`));
      return;
    }
    const child = spawn(helper, [], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => (stdout += chunk.toString('utf8')));
    child.stderr.on('data', (chunk) => (stderr += chunk.toString('utf8')));
    child.on('error', reject);
    child.on('close', () => {
      const line = stdout
        .split(/\r?\n/)
        .filter((entry) => entry.trim().startsWith('{'))
        .map((entry) => {
          try {
            return JSON.parse(entry);
          } catch {
            return null;
          }
        })
        .filter((entry) => entry && entry.type === 'response')
        .pop();
      if (!line) {
        reject(new Error(`no response from llama-helper. stderr tail: ${stderr.slice(-500)}`));
        return;
      }
      if (line.error) {
        reject(new Error(`llama-helper error: ${line.error}`));
        return;
      }
      resolvePromise({ text: line.text ?? '', stderr });
    });
    child.stdin.write(request);
    child.stdin.end();
  });
}

async function runApi(systemPrompt, userPrompt) {
  const response = await fetch(`${apiEndpoint}/chat/completions`, {
    method: 'POST',
    headers: { ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}), 'Content-Type': 'application/json' },
    body: JSON.stringify({
      model: apiModel,
      messages: [
        { role: 'system', content: systemPrompt },
        { role: 'user', content: userPrompt },
      ],
    }),
  });
  if (!response.ok) throw new Error(`api ${response.status}: ${(await response.text()).slice(0, 300)}`);
  const payload = await response.json();
  return { text: payload?.choices?.[0]?.message?.content ?? '', stderr: '' };
}

async function main() {
  const build = spawnSync('cargo', ['build', '--offline', '--manifest-path', join(bridgeDir, 'Cargo.toml')], {
    stdio: 'inherit', windowsHide: true, env: { ...process.env, CARGO_TARGET_DIR: join(bridgeDir, 'target') },
  });
  if (build.status !== 0) throw new Error('无法构建共用协议，请先安装 Rust 并缓存依赖');
  const fixture = JSON.parse(readFileSync(fixturePath, 'utf8'));
  const cases = fixture.cases.filter(entry => args.get('include-historical') === 'true' || entry.annotation_complete === true);
  mkdirSync(outDir, { recursive: true });
  const stamp = new Date().toISOString().replace(/[:.]/g, '-');
  const runners = [];
  if (which === 'local' || which === 'both') runners.push({ name: 'local-qwen3.5-4b', run: runLocal });
  if (which === 'api' || which === 'both') {
    runners.push({ name: `api-${safeName(apiModel)}`, run: runApi });
  }
  if (!runners.length || !cases.length) throw new Error('没有可运行的模型或已完整标注用例');
  let allPass = true;
  for (const runner of runners) for (const testCase of cases) {
    const request = { rows: testCase.segments.map((transcript, i) => ({ id: String(i), transcript, audio_start_time: testCase.starts?.[i] ?? 0 })), context: fixture.context_line };
    const aggregate = { candidates: [], answered: [], dropped: [] };
    const attempts = [];
    let start = 0;
    do {
      const plan = protocol({ ...request, start });
      for (const batch of plan.batches) {
        let indices = batch.indices;
        let prompt = batch.prompt;
        for (let attempt = 0; attempt < 2; attempt++) {
          let response;
          try {
            response = await runner.run(plan.system, prompt);
          } catch (error) {
            aggregate.dropped.push(`request-failed: ${String(error.message ?? error)}`);
            attempts.push({ indices, attempt, error: String(error.message ?? error) });
            break;
          }
          const result = protocol({ ...request, indices, raw: response.text });
          const parsed = result.parsed;
          aggregate.candidates.push(...parsed.candidates);
          aggregate.answered.push(...parsed.answered);
          aggregate.dropped.push(...parsed.dropped);
          attempts.push({ version: plan.version, system: plan.system, prompt, indices, attempt, raw: response.text, parsed });
          if (!parsed.missing.length) break;
          indices = parsed.missing;
          prompt = protocol({ ...request, indices }).prompt;
        }
      }
      start = plan.batches.at(-1)?.indices.at(-1) + 1;
    } while (Number.isFinite(start) && start < request.rows.length);
    const verdict = judge(aggregate, testCase);
    allPass &&= verdict.pass;
    const file = join(outDir, `${stamp}-${runner.name}-${safeName(testCase.id)}.json`);
    writeFileSync(file, JSON.stringify({ id: testCase.id, source: testCase.source, verdict, attempts }, null, 2), 'utf8');
    console.log(`${verdict.pass ? 'PASS' : 'FAIL'} ${runner.name}/${testCase.id}: 命中 ${verdict.hits.length}，漏改 ${verdict.misses.length}，未认可改动 ${verdict.violations.length}，漏答 ${verdict.missingSegments.length}，协议异常 ${verdict.dropped.length}${verdict.incompleteAnnotation ? '，历史用例标注不完整' : ''}`);
  }
  process.exitCode = allPass ? 0 : 1;
}
main().catch(error => { console.error(error); process.exitCode = 1; });
