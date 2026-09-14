import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const read = (path: string) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');

test('summary completion refreshes summary state without taking meeting title ownership', () => {
  const hook = read('src/hooks/meeting-details/useSummaryGeneration.ts');
  const completionStart = hook.indexOf('// Handle successful completion');
  const completionEnd = hook.indexOf('} catch (error)', completionStart);
  assert.ok(completionStart >= 0 && completionEnd > completionStart);

  const completionPath = hook.slice(completionStart, completionEnd);
  assert.match(completionPath, /setAiSummary\(/);
  assert.match(completionPath, /setSummaryStatus\(/);
  assert.doesNotMatch(
    completionPath,
    /updateMeetingTitle|onMeetingUpdated|api_save_meeting_title|setMeetingTitle|setCurrentMeeting/,
  );

  assert.doesNotMatch(hook, /updateMeetingTitle|onMeetingUpdated/);

  const caller = read('src/app/meeting-details/page-content.tsx');
  const hookCallStart = caller.indexOf('const summaryGeneration = useSummaryGeneration({');
  const hookCallEnd = caller.indexOf('});', hookCallStart);
  const hookCall = caller.slice(hookCallStart, hookCallEnd);
  assert.doesNotMatch(hookCall, /updateMeetingTitle|onMeetingUpdated/);
});

test('only the explicit user save path can write a meeting title', () => {
  const meetingData = read('src/hooks/meeting-details/useMeetingData.ts');
  assert.match(meetingData, /const handleSaveMeetingTitle = useCallback/);
  assert.match(meetingData, /invokeTauri\('api_save_meeting_title'/);
  assert.doesNotMatch(meetingData, /const updateMeetingTitle|Updating meeting title to/);

  const service = read('src-tauri/src/summary/service.rs');
  const productionService = service.split(/\r?\n#\[cfg\(test\)\]\r?\nmod tests/)[0];
  assert.match(productionService, /persist_completed_summary\(/);
  assert.doesNotMatch(productionService, /update_meeting_name\(/);
});
