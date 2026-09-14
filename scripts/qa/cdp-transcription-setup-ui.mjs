import fs from 'node:fs';
const [action = 'inspect', output, selector, exactText] = process.argv.slice(2);
if (!['inspect', 'chinese', 'prepare', 'close', 'click', 'choose'].includes(action)) throw new Error('Unknown setup UI action');
if (['click', 'choose'].includes(action) && !selector) throw new Error('Action requires an observed UI selector');
if (action === 'choose' && !exactText) throw new Error('Choose requires an observed option label');
const targets = await fetch(`http://127.0.0.1:${process.env.CDP_PORT ?? '9233'}/json/list`).then(r => r.json());
const page = targets.find(target => target.type === 'page' && target.url.startsWith('http://tauri.localhost'));
if (!page) throw new Error('Actual Meetily WebView2 not found');
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});
const expression = `(async () => {
  const action = ${JSON.stringify(action)};
  const selector = ${JSON.stringify(selector ?? null)};
  const exactText = ${JSON.stringify(exactText ?? null)};
  const visible = element => element.getBoundingClientRect().width > 0 && element.getBoundingClientRect().height > 0;
  const selects = [...document.querySelectorAll('select')].filter(visible);
  const buttons = [...document.querySelectorAll('button')].filter(visible);
  const at = new Date().toISOString();
  if (action !== 'inspect') {
    const state = await window.__TAURI_INTERNALS__.invoke('get_recording_state', {});
    if (state.is_recording || state.is_active) throw new Error('Refusing settings changes during a recording');
    if (action === 'click' || action === 'choose') {
      const matches = [...document.querySelectorAll(selector)].filter(visible).filter(element => action === 'choose' || exactText === null || element.textContent.trim() === exactText);
      if (matches.length !== 1 || matches[0].disabled || matches[0].getAttribute('aria-disabled') === 'true') {
        throw new Error('Expected one enabled visible UI control');
      }
      if (['combobox', 'tab', 'option'].includes(matches[0].getAttribute('role'))) {
        matches[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
      } else matches[0].click();
      if (action === 'choose') {
        await new Promise(resolve => setTimeout(resolve, 200));
        const options = [...document.querySelectorAll('[role="option"]')].filter(visible).filter(element => element.textContent.trim() === exactText);
        if (options.length !== 1 || options[0].getAttribute('aria-disabled') === 'true') throw new Error('Expected one enabled visible option');
        options[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
        await new Promise(resolve => setTimeout(resolve, 300));
        if (matches[0].textContent.trim() !== exactText) throw new Error('UI did not retain selected option');
      }
    } else if (action === 'chinese') {
      const matches = selects.filter(select => [...select.options].some(option => option.value === 'zh'));
      if (matches.length !== 1 || matches[0].disabled) throw new Error('Expected one enabled source-language select');
      matches[0].value = 'zh';
      matches[0].dispatchEvent(new Event('change', { bubbles: true }));
    } else {
      const matches = buttons.filter(button => action === 'prepare'
        ? button.textContent.trim().startsWith('准备中文／多语言识别')
        : button.getAttribute('aria-label') === '关闭转写模型设置');
      if (matches.length !== 1 || matches[0].disabled) throw new Error('Expected exactly one enabled setup action');
      matches[0].click();
    }
  }
  return { action, selector, exactText, at, url: location.href, text: document.body.innerText.slice(-7500),
    options: [...document.querySelectorAll('[role="option"], [role="tab"]')].filter(visible).map(element => ({ text: element.textContent.trim(), role: element.getAttribute('role'), id: element.id, value: element.getAttribute('data-value') })),
    selects: selects.map(select => ({ value: select.value, disabled: select.disabled })),
    buttons: buttons.map(button => ({ text: button.textContent.trim(), label: button.getAttribute('aria-label'), id: button.id, role: button.getAttribute('role'), disabled: button.disabled })) };
})()`;
const response = new Promise((resolve, reject) => {
  socket.addEventListener('message', event => {
    const value = JSON.parse(event.data);
    if (value.id !== 1) return;
    if (value.error || value.result?.exceptionDetails) reject(new Error(JSON.stringify(value.error ?? value.result.exceptionDetails)));
    else resolve(value.result.result.value);
  });
});
socket.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression, returnByValue: true, awaitPromise: true, userGesture: true } }));
try {
  const result = await response;
  if (output) fs.writeFileSync(output, JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result, null, 2));
} finally { socket.close(); }
