'use client';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';

export function RecordingStoragePicker({ onChanged }: { onChanged?: () => void }) {
  const [directory, setDirectory] = useState('');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const lock = useRef(false);
  useEffect(() => {
    let active = true;
    const refresh = () => void invoke<{ save_folder: string }>('get_recording_preferences').then(p => { if (active) setDirectory(p.save_folder); }).catch(e => { if (active) setError(String(e)); });
    refresh();
    const listener = listen('recording-storage-changed', refresh);
    return () => { active = false; void listener.then(unlisten => unlisten()); };
  }, []);
  async function select() {
    if (lock.current) return;
    lock.current = true; setError(''); setMessage('');
    try {
      const selected = await invoke<string | null>('select_recording_folder');
      if (!selected) return;
      setBusy(true);
      const result = await invoke<{ directory: string; message: string }>('migrate_recording_storage', { directory: selected });
      setDirectory(result.directory); setMessage(result.message); onChanged?.();
    } catch (e) { setError(String(e)); }
    finally { lock.current = false; setBusy(false); }
  }
  return <div className="space-y-3">
    <p className="break-all text-xs text-gray-600">{directory || '正在读取目录…'}</p>
    <p className="text-xs text-gray-500">选择空文件夹后迁移录音、转写附件和 Markdown，并更新历史会议路径。校验成功后使用新目录，原文件保留为恢复备份。数据库和 AI 模型位置不变。</p>
    <div className="flex gap-2">
      <Button variant="outline" size="sm" disabled={busy || !directory} onClick={() => void select()}>选择文件夹并迁移</Button>
      <Button variant="outline" size="sm" disabled={busy} onClick={() => void invoke('open_recordings_folder').catch(e => setError(String(e)))}>打开当前目录</Button>
    </div>
    {message && <p role="status" className="text-sm text-green-700">{message}</p>}
    {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
    <Dialog open={busy}>
      <DialogContent onEscapeKeyDown={e => e.preventDefault()} onPointerDownOutside={e => e.preventDefault()}>
        <DialogHeader><DialogTitle>正在迁移会议文件</DialogTitle>
          <DialogDescription>正在复制并校验文件，请保持 Meetily 运行。耗时取决于录音大小；完成前不会切换历史会议路径。</DialogDescription>
        </DialogHeader>
      </DialogContent>
    </Dialog>
  </div>;
}
