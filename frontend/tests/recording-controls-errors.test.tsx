import { afterAll, afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { setUiLanguage } from '../src/i18n';

const originalCore = { ...await import('@tauri-apps/api/core') };
const originalEvent = { ...await import('@tauri-apps/api/event') };
const originalContext = { ...await import('../src/contexts/RecordingStateContext') };
const originalTooltip = { ...await import('../src/components/ui/tooltip') };
const originalAnalytics = { ...await import('../src/lib/analytics') };
mock.module('@tauri-apps/api/core', () => ({ ...originalCore, invoke: async () => false }));
mock.module('@tauri-apps/api/event', () => ({ ...originalEvent, listen: async () => () => {} }));
mock.module('../src/contexts/RecordingStateContext', () => ({ ...originalContext, useRecordingState: () => ({ isPaused: false, isStartingRecording: false }) }));
const Wrapper = ({ children }: any) => children;
mock.module('../src/components/ui/tooltip', () => ({ TooltipProvider: Wrapper, Tooltip: Wrapper, TooltipTrigger: Wrapper, TooltipContent: Wrapper }));
mock.module('../src/lib/analytics', () => ({ ...originalAnalytics, default: { trackButtonClick: () => {} } }));
const { RecordingControls } = await import('../src/components/RecordingControls');
let renderer: ReactTestRenderer | undefined;

afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount());
  renderer = undefined;
  setUiLanguage('zh-CN');
});
afterAll(() => {
  mock.module('@tauri-apps/api/core', () => originalCore);
  mock.module('@tauri-apps/api/event', () => originalEvent);
  mock.module('../src/contexts/RecordingStateContext', () => originalContext);
  mock.module('../src/components/ui/tooltip', () => originalTooltip);
  mock.module('../src/lib/analytics', () => originalAnalytics);
});

async function clickStart(error: string) {
  await act(async () => {
    renderer = create(<RecordingControls isRecording={false} barHeights={[]}
      isRecordingDisabled={false} isParentProcessing={false}
      onRecordingStart={() => { throw new Error(error); }}
      onRecordingStop={() => {}} onTranscriptReceived={() => {}} />);
  });
  await act(async () => renderer!.root.findAllByType('button')[0].props.onClick());
  return JSON.stringify(renderer!.toJSON());
}

test.each([
  ['Microphone access required: denied', '无法访问麦克风'],
  ['需要麦克风访问权限：拒绝访问', '无法访问麦克风'],
  ['system audio unavailable', '无法录制系统声音'],
  ['Permission denied', '需要录音权限'],
  ['Recording start timed out after 120s', '启动录音超时，请重试'],
  ['unrecognized native error', '无法开始录音'],
])('点击开始录音失败后显示中文说明：%s', async (error, expected) => {
  setUiLanguage('zh-CN');
  const content = await clickStart(error);
  expect(content).toContain(expected);
  expect(content).not.toContain('Unable to');
  expect(content).not.toContain('BlackHole');
  await act(async () => renderer!.root.findAllByType('button').find(b => b.props['aria-label'])!.props.onClick());
  expect(JSON.stringify(renderer!.toJSON())).not.toContain(expected);
});

test('英文界面仍显示英文录音错误说明', async () => {
  setUiLanguage('en');
  expect(await clickStart('Microphone not available')).toContain('Unable to access your microphone');
});
