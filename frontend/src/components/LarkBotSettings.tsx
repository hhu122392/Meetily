'use client';

import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import type { LarkConfig, LarkConfigView } from '@/lib/lark';
import { t, useUiLanguage } from "@/i18n";
import { LarkAppSettings } from './LarkAppSettings';

const emptyConfig: LarkConfig = { webhook: '', secret: '', target_name: '' };

export function LarkBotSettings({ onSaved }: { onSaved?: () => void }) {
  useUiLanguage();
  const [config, setConfig] = useState<LarkConfig>(emptyConfig);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [status, setStatus] = useState('');
  const lock = useRef(false);
  useEffect(() => {
    let active = true;
    invoke<LarkConfigView>('lark_get_config').then(value => {
      if (active) { setConfig(value.settings); setLoading(false); }
    }).catch(() => {
      if (active) setError(t("无法读取 Lark 配置，请关闭后重试。"));
    });
    return () => { active = false; };
  }, []);

  async function save(clear = false) {
    if (lock.current) return;
    lock.current = true;
    setBusy(true); setError(''); setStatus('');
    try {
      await invoke(clear ? 'lark_clear_config' : 'lark_save_config', clear ? {} : { config });
      if (clear) setConfig(emptyConfig);
      setStatus(clear ? '已清除本机机器人配置。' : '配置已保存。');
      if (!clear) onSaved?.();
    } catch (error) {
      setError(typeof error === 'string' ? error : t("配置保存失败，请重试。"));
    } finally { lock.current = false; setBusy(false); }
  }

  return <><LarkAppSettings /><section className="space-y-5 py-4 max-w-2xl">
    <div>
      <h2 className="text-lg font-semibold">{t("Lark 群机器人")}</h2>
      <p className="text-sm text-gray-600 mt-2">{t("在 Lark 群设置中添加自定义机器人，将 Webhook 填入下方。保存配置不会发送消息。")}</p>
    </div>
    <div className="space-y-2">
      <Label htmlFor="lark-target">{t("目标群备注")}</Label>
      <Input id="lark-target" value={config.target_name} maxLength={80} disabled={loading || busy}
        placeholder={t("例如：产品周会群")} onChange={e => setConfig({ ...config, target_name: e.target.value })} />
      <p className="text-xs text-gray-500">{t("仅用于发送前确认，请填写该 Webhook 实际所属的群。")}</p>
    </div>
    <div className="space-y-2">
      <Label htmlFor="lark-webhook">{t("Webhook 地址")}</Label>
      <Input id="lark-webhook" type="password" autoComplete="off" value={config.webhook} disabled={loading || busy}
        placeholder="https://open.larksuite.com/open-apis/bot/v2/hook/…"
        onChange={e => setConfig({ ...config, webhook: e.target.value })} />
    </div>
    <div className="space-y-2">
      <Label htmlFor="lark-secret">{t("签名密钥（可选）")}</Label>
      <Input id="lark-secret" type="password" autoComplete="new-password" value={config.secret} disabled={loading || busy}
        placeholder={t("机器人开启签名校验时填写")} onChange={e => setConfig({ ...config, secret: e.target.value })} />
    </div>
    <p className="text-sm text-gray-600">{t("如果机器人启用了关键词校验，请添加“会议纪要”。Webhook 和密钥保存在本机数据库中，请勿分享数据库备份。")}</p>
    {error && <p role="alert" className="text-sm text-red-700">{t(error)}</p>}
    {status && <p role="status" className="text-sm text-green-700">{t(status)}</p>}
    <div className="flex gap-2">
      <Button disabled={loading || busy || !config.webhook.trim() || !config.target_name.trim()} onClick={() => void save()}>
        {busy ? t("正在保存…") : t("保存配置")}
      </Button>
      <Button variant="outline" disabled={loading || busy} onClick={() => void save(true)}>{t("清除配置")}</Button>
    </div>
  </section></>;
}
