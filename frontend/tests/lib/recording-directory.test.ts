import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const read = (path: string) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');

test('recording settings, live recording, and import share the saved-folder contract', () => {
  const settings = read('src/components/RecordingSettings.tsx');
  assert.match(settings, /invoke<RecordingPreferences>\('get_recording_preferences'\)/);
  assert.match(settings, /invoke\('set_recording_preferences', \{ preferences: prefs \}\)/);
  assert.match(settings, /invoke<string \| null>\('select_recording_folder'\)/);

  const commands = read('src-tauri/src/audio/recording_commands.rs');
  assert.equal(
    (commands.match(/resolve_recording_session_preferences\(&app\)/g) ?? []).length,
    2,
  );
  assert.equal(
    (commands.match(/RecordingManager::new\(preferences\.save_folder\.clone\(\)\)/g) ?? [])
      .length,
    2,
  );

  const saver = read('src-tauri/src/audio/recording_saver.rs');
  const productionSaver = saver.slice(0, saver.indexOf('\n#[cfg(test)]\nmod tests'));
  assert.doesNotMatch(productionSaver, /get_default_recordings_folder/);
  assert.match(productionSaver, /recordings_folder: PathBuf/);

  const audioImport = read('src-tauri/src/audio/import.rs');
  const productionImport = audioImport.slice(0, audioImport.indexOf('\n#[cfg(test)]\nmod tests'));
  assert.match(productionImport, /resolve_recording_session_preferences\(&app\)/);
  assert.doesNotMatch(productionImport, /get_default_recordings_folder/);
});

test('stop commands cannot inject a second unrelated save path', () => {
  const controls = read('src/components/RecordingControls.tsx');
  assert.match(controls, /invoke\('stop_recording'\)/);
  assert.doesNotMatch(controls, /appDataDir|save_path|recording-\$\{timestamp\}/);

  const service = read('src/services/recordingService.ts');
  assert.match(service, /async stopRecording\(\): Promise<void>/);
  assert.match(service, /invoke\('stop_recording'\)/);
  assert.doesNotMatch(service, /savePath|save_path/);

  const nativeEntry = read('src-tauri/src/lib.rs');
  const stopStart = nativeEntry.indexOf('async fn stop_recording');
  const stopEnd = nativeEntry.indexOf('\n#[tauri::command]', stopStart + 1);
  const stopCommand = nativeEntry.slice(stopStart, stopEnd);
  assert.doesNotMatch(stopCommand, /RecordingArgs|save_path|create_dir_all/);
});
