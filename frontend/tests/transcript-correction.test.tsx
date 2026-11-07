import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { ReactNode } from 'react';
const original = '请发到vispro';
const candidate = { segment_id: 's1', segment_index: 0, original: 'vispro', suggested: 'V4 Pro', start_char: 3, end_char: 9, segment_text: original, proposed_text: '请发到V4 Pro', reason: 'rules', confidence: 'high', audio_start_time: 0 };
let stale = false; let failModels = false; const refreshed = mock(); const closed = mock();
const invoke = mock(async (name: string, args?: any): Promise<any> => {
  if (name === 'api_apply_meeting_text_corrections') return { edits: [candidate], applied_rules: ['v4_pro'], context_person_count: 0, context_term_count: 0 };
  if (name === 'api_list_proofread_models') { if (failModels) throw 'offline'; return [{ target: { kind: 'summary' }, provider: 'ollama', model: 'test', is_local: true }]; }
  if (name === 'api_get_proofread_target') return { kind: 'summary' };
  if (name === 'api_list_transcript_backups') return { backups: [] };
  if (name === 'api_apply_transcript_proofread_edits') { if (stale) throw 'proofread_source_changed'; return { applied_edits: 1, backup_file: 'before.json', file_sync_pending: false }; }
  if (name === 'api_get_transcript_revision_diff') return { backup_file: args.backupFile };
  if (name === 'api_restore_transcript_revision') return { restored_segments: 1, file_sync_pending: false };
});
mock.module('@tauri-apps/api/core', () => ({ invoke }));
const Container = ({children}: {children?: ReactNode}) => <div>{children}</div>;
mock.module('../src/components/ui/dialog', () => ({ Dialog: Container, DialogContent: Container, DialogHeader: Container, DialogTitle: Container, DialogDescription: Container }));
mock.module('../src/components/MeetingDetails/TranscriptFileSyncNotice', () => ({ TranscriptFileSyncNotice: () => null }));
mock.module('../src/components/MeetingDetails/TranscriptCorrectionPanel', () => ({ TranscriptCorrectionPanel: (p: any) => <><button disabled={p.isApplying || p.isSavingModel} onClick={() => p.onApply(p.candidates)}>应用所选</button><button disabled={!p.aiAvailable} onClick={p.onRunAi}>AI</button></> }));
mock.module('../src/components/MeetingDetails/TranscriptRevisionPanel', () => ({ TranscriptRevisionPanel: (p: any) => <><button onClick={p.onRestore}>恢复</button><button onClick={p.onKeep}>保留</button></> }));
const { TranscriptCorrectionDialog } = await import('../src/components/MeetingDetails/TranscriptCorrectionDialog');
let view: ReactTestRenderer;
const button = (label: string) => view.root.findAllByType('button').find(b => b.children.includes(label))!;
async function mount() { await act(async () => { view = create(<TranscriptCorrectionDialog meetingId="m1" folder="D:/meeting" onComplete={refreshed} onClose={closed} />); }); }
afterEach(async () => { await act(async () => view?.unmount()); invoke.mockClear(); refreshed.mockClear(); closed.mockClear(); stale = failModels = false; });

test('打开只预览规则，确认应用传入原文位置，差异审核结束才刷新页面', async () => {
  await mount();
  expect(invoke.mock.calls).toContainEqual(['api_apply_meeting_text_corrections', { meetingId: 'm1', meetingFolderPath: 'D:/meeting', dryRun: true }]);
  expect(invoke.mock.calls.some(([name]) => name === 'api_apply_transcript_proofread_edits')).toBe(false);
  await act(async () => button('应用所选').props.onClick());
  expect(invoke.mock.calls).toContainEqual(['api_apply_transcript_proofread_edits', { meetingId: 'm1', meetingFolderPath: 'D:/meeting', edits: [{ segment_id: 's1', original: 'vispro', suggested: 'V4 Pro', expected_text: original, start_char: 3, end_char: 9 }] }]);
  expect(refreshed).not.toHaveBeenCalled();
  await act(async () => button('保留').props.onClick());
  expect(refreshed).toHaveBeenCalledTimes(1);
});

test('原文变化时保留候选并显示重查提示，不刷新或关闭窗口', async () => {
  stale = true; await mount();
  await act(async () => button('应用所选').props.onClick());
  expect(JSON.stringify(view.toJSON())).toContain('转写内容已变化');
  expect(refreshed).not.toHaveBeenCalled(); expect(closed).not.toHaveBeenCalled();
});

test('模型目录失败不阻止确认规则候选，AI 按钮不可用', async () => {
  failModels = true; await mount();
  expect(button('应用所选').props.disabled).toBe(false);
  expect(button('AI').props.disabled).toBe(true);
});

test('恢复备份须再次明确确认，打开差异不会恢复', async () => {
  await mount(); await act(async () => button('应用所选').props.onClick());
  await act(async () => button('恢复').props.onClick());
  expect(invoke.mock.calls.some(([name]) => name === 'api_restore_transcript_revision')).toBe(false);
  await act(async () => button('确认恢复转写').props.onClick());
  expect(invoke.mock.calls).toContainEqual(['api_restore_transcript_revision', { meetingId: 'm1', meetingFolderPath: 'D:/meeting', backupFile: 'before.json' }]);
});
