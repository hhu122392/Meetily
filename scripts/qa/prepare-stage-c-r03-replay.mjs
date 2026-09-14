import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

const [ffmpegPath, audioPath, outputDirectory] = process.argv.slice(2);
if (!ffmpegPath || !audioPath || !outputDirectory) {
  throw new Error(
    "Usage: node prepare-stage-c-r03-replay.mjs <ffmpeg.exe> <audio.mp4> <output-directory>",
  );
}

const sourceStartSeconds = 111.6;
const durationSeconds = 600;
const sampleRate = 16000;
fs.mkdirSync(outputDirectory, { recursive: true });
const rawAudioPath = path.resolve(outputDirectory, "mc-r03-real-meeting-10min.f32le");
const result = spawnSync(ffmpegPath, [
  "-hide_banner", "-loglevel", "error", "-y",
  "-ss", String(sourceStartSeconds), "-t", String(durationSeconds),
  "-i", audioPath,
  "-vn", "-ac", "1", "-ar", String(sampleRate), "-f", "f32le", rawAudioPath,
], { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
if (result.status !== 0) {
  throw new Error(`ffmpeg failed: ${result.error?.message ?? result.stderr ?? `status ${result.status}`}`);
}

const audioBytes = fs.readFileSync(audioPath);
const expectedBytes = durationSeconds * sampleRate * 4;
const extractedRawBytes = fs.readFileSync(rawAudioPath);
if (extractedRawBytes.length < expectedBytes) {
  throw new Error(`Expected at least ${expectedBytes} raw bytes, got ${extractedRawBytes.length}`);
}
if (extractedRawBytes.length !== expectedBytes) {
  fs.writeFileSync(rawAudioPath, extractedRawBytes.subarray(0, expectedBytes));
}
const rawBytes = fs.readFileSync(rawAudioPath);
const manifest = {
  preparedAt: new Date().toISOString(),
  sourceAudioPath: path.resolve(audioPath),
  sourceAudioSha256: crypto.createHash("sha256").update(audioBytes).digest("hex"),
  sourceStartSeconds,
  durationSeconds,
  rawAudioPath,
  rawAudioSha256: crypto.createHash("sha256").update(rawBytes).digest("hex"),
  rawByteSize: rawBytes.length,
  sampleRate,
  sampleFormat: "f32le",
  channelCount: 1,
  replayMethod: "The extracted real-meeting audio is fed through the production ContinuousVadProcessor in 100ms capture frames. Real Whisper inference durations are then scheduled against the original playback timeline with the production serial-worker and queued-chunk coalescing rules.",
};
const manifestPath = path.resolve(outputDirectory, "manifest.json");
fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
console.log(manifestPath);
