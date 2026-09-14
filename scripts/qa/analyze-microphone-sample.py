"""Read saved PCM captures; report levels without changing audio or settings."""
import hashlib
import json
import sys
import wave
from pathlib import Path
import numpy as np

results = []
for argument in sys.argv[1:]:
    path = Path(argument)
    with wave.open(str(path), 'rb') as recording:
        if recording.getsampwidth() != 2:
            raise ValueError('Expected 16-bit PCM')
        rate = recording.getframerate()
        channels = recording.getnchannels()
        pcm = np.frombuffer(recording.readframes(recording.getnframes()), dtype='<i2')
    samples = pcm.reshape(-1, channels).astype(np.float64) / 32768
    window = rate // 10
    blocks = samples[:len(samples) // window * window].reshape(-1, window, channels)
    rms = np.sqrt(np.mean(blocks ** 2, axis=(1, 2)))
    db = 20 * np.log10(np.maximum(rms, 1e-15))
    results.append({
        'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
        'sample_rate': rate, 'channels': channels, 'seconds': len(samples) / rate,
        'rms_dbfs': float(20 * np.log10(max(np.sqrt(np.mean(samples ** 2)), 1e-15))),
        'peak_dbfs': float(20 * np.log10(max(np.max(np.abs(samples)), 1e-15))),
        'window_100ms_rms_dbfs_percentiles': dict(zip(['p0','p10','p50','p90','p100'], np.percentile(db,[0,10,50,90,100]).tolist())),
        'stereo_correlation': float(np.corrcoef(samples.T)[0, 1]) if channels == 2 else None,
        'meaning': 'Level statistics only; these do not prove speech recognition or intelligibility.'
    })
print(json.dumps(results, ensure_ascii=True, indent=2))
