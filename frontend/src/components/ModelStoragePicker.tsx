'use client';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';

type Storage = { directory: string; summary_directory: string; alongside_recordings: string; pending_cleanup: number };

export function ModelStoragePicker() {
  const [storage, setStorage] = useState<Storage | null>(null);
  const [target, setTarget] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState('');
  const [error, setError] = useState('');
  const lock = useRef(false);
  useEffect(() => {
    let active = true;
    const refresh = () => void invoke<Storage>('get_model_storage').then(s => { if (active) setStorage(s); }).catch(e => { if (active) setError(String(e)); });
    refresh();
    const progressListener = listen<{done: number; total: number}>('model-storage-progress', e => { if (active) setProgress(`已校验 ${e.payload.done} / ${e.payload.total} 个模型文件`); });
    const recordingListener = listen('recording-storage-changed', refresh);
    return () => { active = false; void progressListener.then(f => f()); void recordingListener.then(f => f()); };
  }, []);
  async function select() {
    if (lock.current) return;
    lock.current = true; setError('');
    try { const selected = await invoke<string | null>('select_model_storage_folder'); if (selected) setTarget(selected); }
    catch (e) { setError(String(e)); }
    finally { lock.current = false; }
  }
  async function migrate() {
    if (!target || lock.current) return;
    lock.current = true; setBusy(true); setError(''); setProgress('正在复制并校验模型文件…');
    try {
      await invoke('migrate_model_storage', {directory: target});
      setProgress('模型已校验，正在重启 Meetily…');
    } catch (e) { setError(String(e)); setBusy(false); lock.current = false; }
  }
  return <div className="space-y-3">
    <p className="break-all text-xs text-gray-600">转写模型：{storage?.directory || '正在读取目录…'}</p>
    <p className="break-all text-xs text-gray-600">纪要模型：{storage?.summary_directory || '正在读取目录…'}</p>
    <p className="text-xs text-gray-500">本机所有公司共用模型。可独立选择保存位置，或放入录音总目录下的 models 文件夹。选择后会迁移已有模型；云端 API 模型不受影响。</p>
    <div className="flex flex-wrap gap-2">
      <Button variant="outline" size="sm" disabled={busy || !storage} onClick={() => void select()}>选择模型文件夹</Button>
      <Button variant="outline" size="sm" disabled={busy || !storage} onClick={() => { setError(''); setTarget(storage!.alongside_recordings); }}>与录音放在同一目录</Button>
    </div>
    {!!storage?.pending_cleanup && <p className="text-xs text-amber-700">原目录还有 {storage.pending_cleanup} 个文件未能安全清理，已保留。重启后会再次校验并尝试清理。</p>}
    {error && !target && <p role="alert" className="text-sm text-red-700">{error}</p>}
    <Dialog open={!!target} onOpenChange={open => { if (!open && !busy) { setTarget(null); setError(''); } }}>
      <DialogContent onEscapeKeyDown={e => { if (busy) e.preventDefault(); }} onPointerDownOutside={e => { if (busy) e.preventDefault(); }}>
        <DialogHeader><DialogTitle>{busy ? '正在迁移模型' : '迁移模型并重启'}</DialogTitle>
          <DialogDescription>请先保存正在编辑的纪要。目标需为空文件夹；复制并校验成功后自动重启，随后清理校验一致的原模型。会议录音和文字记录不变。</DialogDescription>
        </DialogHeader>
        <p className="break-all text-sm">{target}</p>
        {busy && <p role="status">{progress}</p>}
        {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
        {!busy && <div className="flex justify-end gap-2"><Button variant="outline" onClick={() => { setTarget(null); setError(''); }}>取消</Button><Button onClick={() => void migrate()}>确认迁移并重启</Button></div>}
      </DialogContent>
    </Dialog>
  </div>;
}
