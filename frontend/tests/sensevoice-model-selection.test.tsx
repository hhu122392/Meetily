import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
const invoke = mock(async (command: string) => {
  if (command === 'sensevoice_get_available_models') return [{ name: 'sensevoice-small-int8', status: 'Available', size_mb: 239 }];
  if (command === 'parakeet_get_available_models') return [{ name: 'parakeet-tdt-0.6b-v3-int8', status: 'Available', size_mb: 670 }];
  if (command === 'whisper_get_available_models') return [{ name: 'small', status: 'Available', size_mb: 466 }];
  return undefined;
});
mock.module('@tauri-apps/api/core', () => ({ invoke }));
const { useTranscriptionModels } = await import('../src/hooks/useTranscriptionModels');
let result: ReturnType<typeof useTranscriptionModels>;
let renderer: ReactTestRenderer;
function Probe({ provider, model }: { provider: string; model: string }) {
  result = useTranscriptionModels({ provider, model });
  return null;
}
afterEach(() => { renderer?.unmount(); invoke.mockClear(); });
test('已保存的 SenseVoice 配置能在导入与重新转写的模型列表中准确选中', async () => {
  await act(async () => { renderer = create(<Probe provider="sensevoice" model="sensevoice-small-int8" />); });
  await act(async () => { await result.fetchModels(); });
  expect(result.selectedModelKey).toBe('sensevoice:sensevoice-small-int8');
  expect(result.availableModels).toHaveLength(3);
  await act(async () => { result.setSelectedModelKey('whisper:small'); });
  await act(async () => { await result.fetchModels(); });
  expect(result.selectedModelKey).toBe('whisper:small');
});
test('新增默认引擎不会覆盖已有 Parakeet 选择', async () => {
  await act(async () => { renderer = create(<Probe provider="parakeet" model="parakeet-tdt-0.6b-v3-int8" />); });
  await act(async () => { await result.fetchModels(); });
  expect(result.selectedModelKey).toBe('parakeet:parakeet-tdt-0.6b-v3-int8');
});
