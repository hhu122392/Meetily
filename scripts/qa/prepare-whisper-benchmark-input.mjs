// Generate a local IPC input from the fixed fixture; this is not a live-capture test.
import fs from 'node:fs';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const [ffmpeg, audio, output, start = '4', duration = '9'] = process.argv.slice(2);
if (!ffmpeg || !audio || !output) throw new Error('Usage: node prepare-whisper-benchmark-input.mjs <ffmpeg> <audio> <new-output.json> [start-seconds] [duration-seconds]');
if (!Number.isFinite(Number(start)) || Number(start) < 0 || !Number.isFinite(Number(duration)) || Number(duration) <= 0) throw new Error('Invalid time range');
const pcm = execFileSync(ffmpeg, ['-v', 'error', '-i', audio, '-ss', start, '-t', duration, '-ac', '1', '-ar', '16000', '-f', 'f32le', 'pipe:1'], { maxBuffer: 8 * 1024 * 1024 });
const audioData = Array.from({ length: pcm.length / 4 }, (_, index) => pcm.readFloatLE(index * 4));
if (audioData.length !== Math.round(Number(duration) * 16000) || !audioData.every(Number.isFinite)) throw new Error('Unexpected fixture PCM');
fs.writeFileSync(output, JSON.stringify({ command: 'whisper_transcribe_audio', payload: { audioData } }), { flag: 'wx' });
console.log(JSON.stringify({ audio, sourceSha256: createHash('sha256').update(fs.readFileSync(audio)).digest('hex'), startSeconds: Number(start), durationSeconds: Number(duration), samples: audioData.length, output }));
