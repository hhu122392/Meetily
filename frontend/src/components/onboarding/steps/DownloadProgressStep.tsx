import React, { useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Mic, Sparkles, Check, Loader2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { OnboardingContainer } from '../OnboardingContainer';
import { useOnboarding } from '@/contexts/OnboardingContext';
import { SenseVoiceAPI, type SenseVoiceDownloadProgressEvent } from '@/lib/sensevoice';
import { SenseVoiceDownloadSourceSelect } from '@/components/SenseVoiceDownloadSourceSelect';
import { getSenseVoiceDownloadSource, saveSenseVoiceDownloadSource } from '@/lib/sensevoice-download-source';
import { downloadErrorMessage } from '@/lib/download-error';
import { toast } from 'sonner';

const MODEL = 'sensevoice-small-int8';

export function DownloadProgressStep() {
  const { goNext, sensevoiceDownloaded, setSenseVoiceDownloaded, completeOnboarding } = useOnboarding();
  const [source, setSource] = useState(getSenseVoiceDownloadSource);
  const [isMac, setIsMac] = useState(false);
  const [ready, setReady] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [isCompleting, setIsCompleting] = useState(false);
  const [error, setError] = useState('');
  const [progress, setProgress] = useState({ percent: 0, downloaded: 0, total: 239, speed: 0 });
  const busy = useRef(false);

  useEffect(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];
    const register = async <T,>(event: string, handler: (payload: T) => void) => {
      const unlisten = await listen<T>(event, ({ payload }) => { if (!disposed) handler(payload); });
      if (disposed) unlisten(); else unlisteners.push(unlisten);
    };
    const initialize = async () => {
      try {
        const { platform } = await import('@tauri-apps/plugin-os');
        if (!disposed) setIsMac(platform() === 'macos');
      } catch { if (!disposed) setIsMac(navigator.userAgent.includes('Mac')); }
      try {
        await register<SenseVoiceDownloadProgressEvent>('sensevoice-model-download-progress', event => {
          if (event.modelName !== MODEL) return;
          if (event.status === 'cancelled') {
            busy.current = false;
            setDownloading(false);
            setCancelling(false);
            return;
          }
          setProgress({ percent: event.progress, downloaded: event.downloaded_mb ?? 0, total: event.total_mb ?? 239, speed: event.speed_mbps ?? 0 });
          // Only the completion event follows successful integrity verification.
        });
        await register<{ modelName: string }>('sensevoice-model-download-complete', event => {
          if (event.modelName !== MODEL) return;
          setSenseVoiceDownloaded(true);
          setDownloading(false);
          setCancelling(false);
          busy.current = false;
        });
        await register<{ modelName: string; error: string }>('sensevoice-model-download-error', event => {
          if (event.modelName !== MODEL) return;
          setError(downloadErrorMessage(event.error));
          setDownloading(false);
          setCancelling(false);
          busy.current = false;
        });
        await SenseVoiceAPI.init();
        const models = await SenseVoiceAPI.getAvailableModels();
        if (disposed) return;
        const model = models.find(model => model.name === MODEL);
        setSenseVoiceDownloaded(model?.status === 'Available');
        const active = !!model && typeof model.status === 'object' && 'Downloading' in model.status;
        busy.current = active;
        setDownloading(active);
        setReady(true);
      } catch (error) { if (!disposed) setError(downloadErrorMessage(error)); }
    };
    void initialize();
    return () => { disposed = true; unlisteners.forEach(unlisten => unlisten()); };
  }, []);

  const startDownload = async () => {
    if (busy.current) return;
    busy.current = true;
    setDownloading(true);
    setError('');
    try {
      await SenseVoiceAPI.downloadModel(MODEL, source);
    } catch (error) {
      setError(downloadErrorMessage(error));
    } finally {
      busy.current = false;
      setDownloading(false);
      setCancelling(false);
    }
  };

  const cancelDownload = async () => {
    setCancelling(true);
    try {
      const outcome = await SenseVoiceAPI.cancelDownload(MODEL);
      if (outcome === 'cancelled') {
        busy.current = false;
        setDownloading(false);
        setCancelling(false);
      }
    } catch (error) { setError(downloadErrorMessage(error)); setCancelling(false); }
  };

  const handleContinue = async () => {
    setIsCompleting(true);
    try {
      if (!await SenseVoiceAPI.hasAvailableModels()) {
        setSenseVoiceDownloaded(false);
        throw new Error('请先完成 SenseVoice 转写模型下载。');
      }
      if (isMac) { goNext(); return; }
      await completeOnboarding();
      window.location.reload();
    } catch (error) { toast.error('无法完成设置', { description: downloadErrorMessage(error) }); }
    finally { setIsCompleting(false); }
  };

  return <OnboardingContainer title="准备转写模型" description="选择下载来源，安装 SenseVoice 后即可开始使用。纪要模型不会自动下载。" step={3} totalSteps={isMac ? 4 : 3}>
    <div className="mx-auto w-full max-w-lg space-y-5">
      <SenseVoiceDownloadSourceSelect value={source} disabled={downloading || cancelling}
        onChange={value => { setSource(value); saveSenseVoiceDownloadSource(value); }} />
      <div className="space-y-4 rounded-xl border border-gray-200 bg-white p-5">
        <div className="flex items-center gap-3">
          <Mic className="h-6 w-6 text-gray-600" />
          <div className="flex-1"><h3 className="font-medium">SenseVoiceSmall 转写引擎</h3><p className="text-sm text-gray-500">约 239 MB · 中文、粤语及多语言 · 本地离线</p></div>
          {sensevoiceDownloaded && <Check className="h-6 w-6 text-green-600" />}
        </div>
        {downloading && <div className="space-y-2">
          <div className="h-2 overflow-hidden rounded-full bg-gray-200"><div className="h-full bg-gray-800" style={{ width: `${progress.percent}%` }} /></div>
          <p className="text-sm text-gray-600">{progress.downloaded.toFixed(1)} / {progress.total.toFixed(1)} MB · {progress.speed.toFixed(1)} MB/s · {progress.percent}%</p>
        </div>}
        {error && <p role="alert" className="break-words rounded-md bg-red-50 p-3 text-sm text-red-600">{error}</p>}
        {!sensevoiceDownloaded && (downloading ?
          <Button variant="outline" onClick={cancelDownload} disabled={cancelling}>{cancelling ? '正在取消…' : '取消下载'}</Button> :
          <Button onClick={startDownload} disabled={!ready}>{!ready ? '正在初始化…' : error ? '重试下载' : '下载 SenseVoice'}</Button>)}
        {sensevoiceDownloaded && <p className="text-sm text-green-700">转写模型已安装</p>}
      </div>
      <div className="rounded-xl border border-gray-200 bg-white p-5">
        <h3 className="flex items-center gap-3 font-medium"><Sparkles className="h-5 w-5 text-gray-600" />纪要模型 · 稍后自行选择</h3>
        <p className="mt-3 text-sm leading-6 text-gray-600">安装时不下载纪要模型。进入软件后，在“设置 → 纪要”选择本地模型并点击下载，或配置自己的 AI 接口。已有模型和接口设置会保留。</p>
      </div>
      <Button className="w-full" disabled={!sensevoiceDownloaded || isCompleting} onClick={handleContinue}>
        {isCompleting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}继续（不下载纪要模型）
      </Button>
    </div>
  </OnboardingContainer>;
}
