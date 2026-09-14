import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

const [ffmpegPath, audioPath, transcriptPath, outputDirectory] = process.argv.slice(2);
if (!ffmpegPath || !audioPath || !transcriptPath || !outputDirectory) {
  throw new Error(
    "Usage: node prepare-stage-c-r02-clips.mjs <ffmpeg.exe> <audio.mp4> <transcripts.json> <output-directory>",
  );
}

const selectedSequenceIds = [6, 8, 10, 12, 15, 17, 20, 23, 24, 25];
const transcriptBytes = fs.readFileSync(transcriptPath);
const transcript = JSON.parse(transcriptBytes.toString("utf8"));
const segments = new Map(transcript.segments.map((segment) => [segment.sequence_id, segment]));
fs.mkdirSync(outputDirectory, { recursive: true });

const clips = selectedSequenceIds.map((sequenceId, index) => {
  const segment = segments.get(sequenceId);
  if (!segment) throw new Error(`Transcript segment ${sequenceId} was not found`);
  const startSeconds = segment.audio_start_time + 0.25;
  const durationSeconds = Math.min(8, Math.max(2.5, segment.duration - 0.5));
  const clipPath = path.resolve(outputDirectory, `mc-r02-${String(index + 1).padStart(2, "0")}-seq-${sequenceId}.f32le`);
  const result = spawnSync(ffmpegPath, [
    "-hide_banner", "-loglevel", "error", "-y",
    "-ss", String(startSeconds), "-t", String(durationSeconds),
    "-i", audioPath,
    "-vn", "-ac", "1", "-ar", "16000", "-f", "f32le", clipPath,
  ], { encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`ffmpeg failed for sequence ${sequenceId}: ${result.stderr}`);
  }
  const bytes = fs.readFileSync(clipPath);
  if (bytes.length < 16000 * 4 * 2) throw new Error(`Clip ${sequenceId} is unexpectedly short`);
  return {
    clipId: `mc-r02-${String(index + 1).padStart(2, "0")}`,
    sequenceId,
    path: clipPath,
    startSeconds,
    durationSeconds,
    byteSize: bytes.length,
    sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
    sourceTranscriptTextSha256: crypto.createHash("sha256").update(segment.text, "utf8").digest("hex"),
  };
});

const manifest = {
  preparedAt: new Date().toISOString(),
  sourceAudioPath: path.resolve(audioPath),
  sourceAudioSha256: crypto.createHash("sha256").update(fs.readFileSync(audioPath)).digest("hex"),
  sourceTranscriptPath: path.resolve(transcriptPath),
  sourceTranscriptSha256: crypto.createHash("sha256").update(transcriptBytes).digest("hex"),
  sampleRate: 16000,
  sampleFormat: "f32le",
  channelCount: 1,
  selectionRule: "Ten real meeting segments whose saved transcript contains none of the configured canonical names, aliases, or terms; each clip is capped at eight seconds.",
  clips,
};
const manifestPath = path.resolve(outputDirectory, "manifest.json");
fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
console.log(manifestPath);
