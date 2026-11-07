'use client';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { TranscriptCorrectionPanel } from './TranscriptCorrectionPanel';
import { TranscriptRevisionPanel } from './TranscriptRevisionPanel';
import { TranscriptFileSyncNotice } from './TranscriptFileSyncNotice';
import { transcriptCandidateKey } from '@/lib/transcript-edit-selection';
import { applyMeetingTextCorrections, applyTranscriptProofreadEdits, getProofreadTarget, getTranscriptRevisionDiff, listProofreadModels, listTranscriptBackups, restoreTranscriptRevision, reviewTranscriptWithLLM, setProofreadTarget, type CorrectionCandidate, type ProofreadCandidate, type ProofreadModelOption, type ProofreadResponse, type ProofreadTarget, type TextCorrectionResponse, type TranscriptBackupInfo, type TranscriptDiffResponse } from '@/lib/transcript-revision';

function correctionError(error: unknown): string {
  const text = String(error);
  const messages: Record<string, string> = {
    proofread_source_changed: '转写内容已变化，请关闭纠错窗口后重新检查。',
    proofread_overlapping_edits: '选中的修改相互重叠，请每个位置只保留一项。',
    proofread_invalid_range: '纠错位置与原文不一致，请重新检查。',
    proofread_model_unavailable: '所选校对模型不可用，请检查模型配置或选择其他模型。',
    transcript_files_pending: '转写文件尚未同步，请先重试文件同步。',
  };
  return messages[text] ?? text;
}

export function TranscriptCorrectionDialog({ meetingId, folder, onClose, onComplete }: {
  meetingId: string; folder: string; onClose: () => void; onComplete?: () => void;
}) {
  const [rules, setRules] = useState<TextCorrectionResponse | null>(null);
  const [runs, setRuns] = useState<ProofreadResponse[]>([]);
  const [models, setModels] = useState<ProofreadModelOption[]>([]);
  const [target, setTarget] = useState<ProofreadTarget>({ kind: 'summary' });
  const [modelReady, setModelReady] = useState(false);
  const [busy, setBusy] = useState<string | null>('loading');
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [diff, setDiff] = useState<TranscriptDiffResponse | null>(null);
  const [backups, setBackups] = useState<TranscriptBackupInfo[]>([]);
  const [confirmRestore, setConfirmRestore] = useState(false);
  const changed = useRef(false);
  const locked = useRef(false);
  const active = useRef(true);

  useEffect(() => {
    active.current = true;
    void (async () => {
      const results = await Promise.allSettled([
        applyMeetingTextCorrections(meetingId, folder, true),
        Promise.all([listProofreadModels(), getProofreadTarget()]),
        listTranscriptBackups(meetingId, folder),
      ]);
      if (!active.current) return;
      if (results[0].status === 'fulfilled') setRules(results[0].value);
      else setError(correctionError(results[0].reason));
      if (results[1].status === 'fulfilled') {
        setModels(results[1].value[0]); setTarget(results[1].value[1]); setModelReady(true);
      } else setNotice('校对模型未能加载，仍可查看规则候选；关闭后重试模型加载。');
      if (results[2].status === 'fulfilled') setBackups(results[2].value.backups);
      setBusy(null);
    })();
    return () => { active.current = false; };
  }, [meetingId, folder]);

  const candidates = useMemo(() => {
    const seen = new Set<string>(); const result: CorrectionCandidate[] = [];
    const all: CorrectionCandidate[] = [
      ...(rules?.edits ?? []).map(edit => ({ ...edit, sourceKind: 'rules' as const })),
      ...runs.flatMap(run => run.candidates.map(edit => ({ ...edit, sourceKind: (['builtin-ai', 'ollama'].includes(run.provider) ? 'local' : 'api') as 'local' | 'api' }))),
    ];
    for (const edit of all) { const key = transcriptCandidateKey(edit); if (!seen.has(key)) { seen.add(key); result.push(edit); } }
    return result;
  }, [rules, runs]);

  const close = () => {
    if (busy || locked.current) return;
    if (changed.current) onComplete?.();
    onClose();
  };
  const work = async (kind: string, action: () => Promise<void>) => {
    if (locked.current || busy) return;
    locked.current = true; setBusy(kind); setError('');
    try { await action(); } catch (e) { if (active.current) setError(correctionError(e)); }
    finally { locked.current = false; if (active.current) setBusy(null); }
  };
  const runAi = () => void work('ai', async () => {
    if (!modelReady) throw new Error('校对模型尚未就绪');
    const previous = runs.at(-1);
    const continuation = previous && previous.next_start_index !== null && JSON.stringify(previous.target) === JSON.stringify(target);
    const result = await reviewTranscriptWithLLM(meetingId, folder, target, continuation ? previous.next_start_index! : 0, continuation ? previous.source_hash : undefined);
    setRuns(old => [...old, result]);
  });
  const apply = (selected: ProofreadCandidate[]) => void work('apply', async () => {
    const result = await applyTranscriptProofreadEdits(meetingId, folder, selected.map(edit => ({ segment_id: edit.segment_id, original: edit.original, suggested: edit.suggested, expected_text: edit.segment_text, start_char: edit.start_char, end_char: edit.end_char })));
    changed.current = true; setRules(null); setRuns([]);
    setNotice(`已应用 ${result.applied_edits} 处纠错。已有纪要保持原样，需要时可重新生成。${result.file_sync_pending ? '转写文件待同步，请点击重试。' : ''}`);
    if (result.backup_file) setDiff(await getTranscriptRevisionDiff(meetingId, folder, result.backup_file));
    setBackups((await listTranscriptBackups(meetingId, folder)).backups);
  });
  const restore = () => void work('restore', async () => {
    if (!diff) return;
    const result = await restoreTranscriptRevision(meetingId, folder, diff.backup_file);
    changed.current = true; setConfirmRestore(false); setDiff(null); setRules(null); setRuns([]);
    setNotice(`已恢复 ${result.restored_segments} 个转写片段。${result.file_sync_pending ? '文件待同步，请重试。' : ''}`);
    setBackups((await listTranscriptBackups(meetingId, folder)).backups);
  });

  return <Dialog open onOpenChange={value => { if (!value) close(); }}>
    <DialogContent className="flex max-h-[90vh] w-[calc(100vw-2rem)] max-w-5xl flex-col overflow-hidden rounded-2xl" onEscapeKeyDown={event => { if (busy) event.preventDefault(); }} onInteractOutside={event => { if (busy) event.preventDefault(); }}>
      <DialogHeader><DialogTitle>转写文字纠错</DialogTitle><DialogDescription>先检查术语规则，也可使用所选模型逐段校对。勾选确认后才修改转写，修改前自动备份；不改动已有纪要。</DialogDescription></DialogHeader>
      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto pr-1">
        <TranscriptFileSyncNotice meetingId={meetingId} revision={notice} />
        {error && <p role="alert" className="break-words rounded-lg bg-red-50 p-3 text-sm text-red-700">{error}</p>}
        {notice && <p role="status" className="rounded-lg bg-blue-50 p-3 text-sm">{notice}</p>}
        {busy === 'loading' && <p role="status">正在检查规则和加载模型…</p>}
        {!!backups.length && <label className="block text-sm">历史转写版本<select aria-label="转写备份版本" disabled={!!busy} value={diff?.backup_file ?? ''} className="ml-2 max-w-full rounded-lg border p-2" onChange={event => { const file = event.target.value; if (file) void work('history', async () => { setConfirmRestore(false); setDiff(await getTranscriptRevisionDiff(meetingId, folder, file)); }); }}><option value="">选择备份查看差异</option>{backups.map(backup => <option key={backup.file} value={backup.file}>{backup.saved_at ?? backup.created_at ?? backup.file} · {backup.total_segments} 段</option>)}</select></label>}
        {diff ? <>
          <TranscriptRevisionPanel diff={diff} isRestoring={busy === 'restore'} onRestore={() => setConfirmRestore(true)} onKeep={close} />
          {confirmRestore && <div className="rounded-lg border bg-amber-50 p-3 text-sm"><p>确认恢复此版本？当前转写会先备份，已有纪要保持原样。</p><div className="mt-2 flex gap-2"><Button disabled={!!busy} onClick={restore}>确认恢复转写</Button><Button variant="outline" disabled={!!busy} onClick={() => setConfirmRestore(false)}>取消</Button></div></div>}
        </> : rules && <TranscriptCorrectionPanel candidates={candidates} ruleCandidateCount={rules.edits.length} appliedRules={rules.applied_rules} contextPersonCount={rules.context_person_count} contextTermCount={rules.context_term_count} aiRuns={runs} isRunningAi={busy === 'ai'} isApplying={busy === 'apply'} isSavingModel={busy === 'model'} aiAvailable={modelReady && models.length > 0} modelOptions={models} selectedTarget={target} onRunAi={runAi} onTargetChange={next => void work('model', async () => { await setProofreadTarget(next); setTarget(next); })} onApply={apply} onCancel={close} />}
      </div>
      <div className="flex shrink-0 justify-end border-t pt-3"><Button variant="outline" disabled={!!busy} onClick={close}>关闭</Button></div>
    </DialogContent>
  </Dialog>;
}
