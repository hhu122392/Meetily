import fs from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';

// Read-only geometry evidence from the actual client; never inserts transcripts.
const [output, duration = '0'] = process.argv.slice(2);
if (!output) throw new Error('Usage: cdp-recording-layout-check.mjs <output.json> [duration-ms|--audit]');
let samples;
if (duration === '--audit') {
  samples = JSON.parse(fs.readFileSync(output, 'utf8').replace(/^\uFEFF/, '')).samples;
} else {
  const targets = await fetch(`http://127.0.0.1:${process.env.CDP_PORT}/json/list`).then(r => r.json());
  const target = targets.find(t => t.id === process.env.CDP_TARGET_ID && t.type === 'page' && t.url.startsWith('http://tauri.localhost'));
  if (!target) throw new Error('Exact process-bound Tauri target required');
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', reject, { once: true });
  });
  const expression = `(() => {
    const rect = e => e?.getBoundingClientRect().toJSON() ?? null;
    const meters = [...document.querySelectorAll('[role="meter"]')];
    const card = meters.at(-1)?.closest('div[aria-live="polite"]');
    const stop = document.querySelector('button[aria-label="停止录音"]');
    const controls = stop?.closest('.flex.flex-col.items-center.gap-2');
    const ancestors = [];
    for (let e = meters[0]?.parentElement; e; e = e.parentElement) {
      const css = getComputedStyle(e);
      ancestors.push({ class: e.className, overflowY: css.overflowY, position: css.position,
        scrollTop: e.scrollTop, scrollHeight: e.scrollHeight, clientHeight: e.clientHeight, rect: rect(e) });
    }
    const scroller = ancestors.find(e => e.overflowY === 'auto' && e.scrollHeight > e.clientHeight + 1);
    return { at: new Date().toISOString(), viewport: { width: innerWidth, height: innerHeight },
      meters: meters.map(e => ({ label: e.getAttribute('aria-label'), value: +e.getAttribute('aria-valuenow'), rect: rect(e) })),
      card: rect(card), controls: rect(controls), ancestors,
      distanceFromBottom: scroller ? scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight : 0,
      segmentIds: [...document.querySelectorAll('[data-transcript-id]')].map(e => e.dataset.transcriptId) };
  })()`;
  samples = [];
  const deadline = Date.now() + Number(duration);
  try {
    do {
      const snapshot = await new Promise((resolve, reject) => {
        const timer = setTimeout(() => { socket.removeEventListener('message', handler); reject(new Error('Geometry timeout')); }, 5000);
        function handler(event) {
          const result = JSON.parse(event.data);
          if (result.id !== 1) return;
          clearTimeout(timer);
          socket.removeEventListener('message', handler);
          if (result.error || result.result?.exceptionDetails) reject(new Error(JSON.stringify(result)));
          else resolve(result.result.result.value);
        }
        socket.addEventListener('message', handler);
        socket.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression, returnByValue: true } }));
      });
      samples.push(snapshot);
      fs.writeFileSync(output, JSON.stringify({ target: target.id, samples }));
      if (Date.now() < deadline) await delay(1000);
    } while (Date.now() < deadline);
  } finally { socket.close(); }
}
if (!samples?.length) throw new Error('No geometry samples');
const topFailures = samples.filter(s => s.meters.length < 2 || s.meters.slice(0, 2).some(m => m.rect.top < 0 || m.rect.bottom > (s.viewport?.height ?? Infinity)));
const equalFailures = samples.filter(s => s.meters.length === 4 && (s.meters[0].value !== s.meters[2].value || s.meters[1].value !== s.meters[3].value));
const bottomSamples = samples.filter(s => s.card && s.controls && s.distanceFromBottom <= 2);
const longBottomSamples = bottomSamples.filter(s => s.segmentIds.length >= 10);
const cardFailures = samples.filter(s => s.card && s.controls && (
  s.card.bottom > Math.min(s.controls.top, s.viewport.height,
    s.ancestors.find(e => e.overflowY === 'auto')?.rect.bottom ?? Infinity) + 1
  || s.card.top < s.meters[0].rect.bottom));
const result = { samples: samples.length, topFailures: topFailures.length, equalFailures: equalFailures.length,
  bottomSamples: bottomSamples.length, longBottomSamples: longBottomSamples.length, cardFailures: cardFailures.length,
  firstFailure: topFailures[0]?.at ?? cardFailures[0]?.at ?? null };
console.log(JSON.stringify(result));
if (topFailures.length || equalFailures.length || cardFailures.length || !longBottomSamples.length) process.exitCode = 1;
