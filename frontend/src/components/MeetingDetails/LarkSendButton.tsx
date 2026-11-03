'use client';

import { useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Send, Loader2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { LarkBotSettings } from '@/components/LarkBotSettings';
import { type LarkConfigView, type LarkSendResult } from '@/lib/lark';
import { t, useUiLanguage } from "@/i18n";

interface Props {
  meetingId: string;
  title: string;
  createdAt: string;
  getSummary: () => Promise<string>;
  disabled?: boolean;
}

export function LarkSendButton({ meetingId, title, createdAt, getSummary, disabled }: Props) {
  useUiLanguage();
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [sending, setSending] = useState(false);
  const [config, setConfig] = useState<LarkConfigView | null>(null);
  const [summary, setSummary] = useState('');
  const [previewTitle, setPreviewTitle] = useState('');
  const [error, setError] = useState('');
  const [result, setResult] = useState<LarkSendResult | null>(null);
  const requestVersion = useRef(0);
  const lock = useRef(false);
  const parsedDate = new Date(createdAt);
  const date = Number.isNaN(parsedDate.getTime()) ? createdAt : parsedDate.toLocaleString('zh-CN');

  async function prepare() {
    if (lock.current) return;
    const version = ++requestVersion.current;
    setOpen(true); setLoading(true); setError(''); setResult(null); setConfig(null); setSummary('');
    setPreviewTitle(title);
    try {
      const [settings, text] = await Promise.all([
        invoke<LarkConfigView>('lark_get_config'), getSummary(),
      ]);
      if (version !== requestVersion.current) return;
      setConfig(settings);
      if (!text.trim()) throw new Error('纪要内容为空或尚未加载，请稍后重试。');
      setSummary(text);
    } catch {
      if (version === requestVersion.current) setError(t("无法准备发送内容，请确认纪要已加载，并使用包含 Lark 功能的桌面版本。"));
    } finally {
      if (version === requestVersion.current) setLoading(false);
    }
  }

  async function send() {
    if (lock.current || !config || !summary.trim() || result) return;
    lock.current = true; setSending(true); setError('');
    try {
      const value = await invoke<LarkSendResult>('lark_send_summary', {
        meetingId, title: previewTitle, date, summary, expectedConfigRevision: config.revision,
      });
      setResult(value);
    } catch (error) {
      setError(typeof error === 'string' ? error : t("发送失败，请检查群内消息后再决定是否重试。"));
    } finally { lock.current = false; setSending(false); }
  }

  return <>
    <Button variant="outline" size="sm" onClick={() => void prepare()} disabled={disabled || loading || sending}>
      <Send size={16} />Webhook 发送 </Button>
    <Dialog open={open} onOpenChange={next => {
      if (lock.current) return;
      if (!next) ++requestVersion.current;
      setOpen(next);
    }}>
      <DialogContent className="max-w-2xl max-h-[85vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{t("发送会议纪要到 Lark")}</DialogTitle>
          <DialogDescription>{t("确认目标群和内容后发送。预览中的修改只影响本次消息，不会修改原纪要。")}</DialogDescription>
        </DialogHeader>
        {loading ? <p role="status">{t("正在准备纪要…")}</p> : config && !config.settings.webhook ?
          <LarkBotSettings onSaved={() => void prepare()} /> : config && <>
            <p className="text-sm">{t("目标群：")}<strong>{config.settings.target_name}</strong></p>
            <p className="text-sm font-medium">{t("会议纪要：")}{previewTitle}</p>
            <p className="text-xs text-gray-500">{t("会议时间：")}{date}</p>
            <label htmlFor="lark-summary" className="text-sm">{t("消息正文（纯文本）")}</label>
            <textarea id="lark-summary" className="border rounded-md p-3 w-full min-h-64 text-sm" value={summary}
              disabled={sending || !!result} onChange={e => setSummary(e.target.value)} />
            <p className="text-xs text-gray-500">{t("正文将连同上方的标题和时间一起发送。超长纪要会提示精简，不会自动截断。")}</p>
            <Button onClick={() => void send()} disabled={sending || !!result || !summary.trim()}
              aria-label={t("确认发送到 Lark")}>
              {sending && <Loader2 className="animate-spin" size={16} />}
              {sending ? t("正在发送…") : result ? t("已发送") : t("确认发送")}
            </Button>
          </>}
        {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
        {result && <p role="status" className="text-sm text-green-700">
          {result.already_sent ? t("相同内容已发送到该机器人，本次未重复发送。") : t("纪要已成功发送到 Lark。")}
        </p>}
      </DialogContent>
    </Dialog>
  </>;
}
