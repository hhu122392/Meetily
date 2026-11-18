import { afterAll, afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { testInvoke as invoke, setTestInvoke, restoreTestInvoke } from '../lib/tauri-test-invoke';
const indexed = { ...await import('../../src/services/indexedDBService') };
const storage = { ...await import('../../src/services/storageService') };
const language = { ...await import('../../src/lib/summary-language-preferences') };
mock.module('../../src/services/indexedDBService', () => ({ ...indexed, indexedDBService: {
  getMeetingMetadata: async () => ({ title: '恢复测试', folderPath: 'C:/recording' }),
  getTranscripts: async () => [{ id: 1, text: '已识别文字', sequenceId: 1 }],
  markMeetingSaved: async () => {},
} }));
mock.module('../../src/services/storageService', () => ({ ...storage, storageService: { saveMeeting: async () => ({ meeting_id: 'recovered' }) } }));
mock.module('../../src/lib/summary-language-preferences', () => ({ ...language, applyPinnedSummaryLanguageToMeeting: async () => {} }));
const { useTranscriptRecovery } = await import('../../src/hooks/useTranscriptRecovery');
let hook: ReturnType<typeof useTranscriptRecovery>;
function Probe() { hook = useTranscriptRecovery(); return null; }
let renderer: ReactTestRenderer | undefined;
afterEach(async () => { if (renderer) await act(async () => renderer!.unmount()); renderer = undefined; invoke.mockClear(); });
afterAll(() => {
  restoreTestInvoke();
  mock.module('../../src/services/indexedDBService', () => indexed);
  mock.module('../../src/services/storageService', () => storage);
  mock.module('../../src/lib/summary-language-preferences', () => language);
});
for (const status of ['failed', 'partial', 'none', 'success']) {
  test(`音频恢复 ${status} 时只允许完整成功后清理片段`, async () => {
    setTestInvoke(async command => command === 'recover_audio_from_checkpoints'
      ? { status, chunk_count: 2, estimated_duration_seconds: 60, message: status } : undefined);
    await act(async () => { renderer = create(<Probe />); });
    let result: Awaited<ReturnType<typeof hook.recoverMeeting>> | undefined;
    await act(async () => { result = await hook.recoverMeeting('original'); });
    expect(result?.success).toBe(true);
    expect(result?.audioRecoveryStatus?.status).toBe(status);
    expect(invoke.mock.calls.some(call => call[0] === 'cleanup_checkpoints')).toBe(status === 'success');
  });
}
