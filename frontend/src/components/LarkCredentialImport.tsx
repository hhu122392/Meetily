'use client';

import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { isLarkAppId, larkCredentialsUrl, parseLarkCredentials, type LarkCredentials } from '@/lib/lark-credentials';

type Props = {
  appId: string;
  secretReady: boolean;
  disabled: boolean;
  onImport: (credentials: LarkCredentials) => void;
};

export function LarkCredentialImport({ appId, secretReady, disabled, onImport }: Props) {
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const revision = useRef(0);
  useEffect(() => { revision.current++; }, [appId, disabled]);
  useEffect(() => () => { revision.current++; }, []);

  function importText(text: string) {
    setError('');
    try {
      const credentials = parseLarkCredentials(text);
      if (!credentials.appId && !isLarkAppId(appId.trim())) {
        setError('请先复制并粘贴 App ID，再复制 App Secret。');
        return;
      }
      onImport(credentials);
    } catch (e) { setError(e instanceof Error ? e.message : '无法识别凭证，请重新复制。'); }
  }

  async function readClipboard() {
    if (disabled || pending.current) return;
    pending.current = true; setBusy(true); setError('');
    const current = revision.current;
    try {
      const text = await navigator.clipboard.readText();
      if (current === revision.current) importText(text);
    } catch {
      if (current === revision.current) setError('无法读取剪贴板，请点击粘贴框按 Ctrl+V；Mac 按 ⌘V。');
    } finally { pending.current = false; setBusy(false); }
  }

  return <details className="rounded-lg border bg-gray-50 p-3 text-sm">
    <summary className="cursor-pointer font-medium">后台辅助导入（免手填）</summary>
    <div className="mt-3 space-y-3">
      <p>打开后台 → 复制凭证 → 保存配置</p>
      <Button type="button" variant="outline" disabled={disabled || busy} onClick={async () => {
        setError('');
        try { await invoke('open_external_url', { url: larkCredentialsUrl(appId) }); }
        catch { setError('无法打开浏览器，请访问 open.larksuite.com/app，进入“凭证与基础信息”。'); }
      }}>打开 Lark 后台</Button>
      <p className="text-xs text-gray-600">用有应用管理权限的账号登录，选择会议助手，进入“凭证与基础信息”。分别点击 App ID 和 App Secret 旁的复制按钮，每复制一项就回到这里粘贴。</p>
      <div className="flex flex-wrap items-center gap-2">
        <Input id="lark-credential-paste" aria-label="粘贴后台凭证" type="password" autoComplete="off"
          className="min-w-0 flex-1" placeholder="在此粘贴，自动识别并填入" value="" onChange={() => {}}
          disabled={disabled || busy} onPaste={event => {
            event.preventDefault();
            if (!disabled && !busy) importText(event.clipboardData.getData('text/plain'));
          }} />
        <Button type="button" variant="outline" disabled={disabled || busy} onClick={() => void readClipboard()}>
          {busy ? '正在读取…' : '读取剪贴板'}
        </Button>
      </div>
      <p role="status" className="text-xs text-gray-600">
        App ID：{isLarkAppId(appId.trim()) ? appId.trim() : '待粘贴'} · 密钥：{secretReady ? '已填入或已保存' : '待粘贴'}。
        {isLarkAppId(appId.trim()) && secretReady ? ' 请核对下方配置，点击“保存并连接”。' : ''}
      </p>
      <p className="text-xs text-gray-500">已有 App ID 时只需粘贴密钥；两项须来自同一个应用。导入后尚未保存，不会自动开启录音或发群。</p>
      {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
    </div>
  </details>;
}
