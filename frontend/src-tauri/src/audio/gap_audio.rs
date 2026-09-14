//! Keep only the real PCM needed to reconnect short VAD gaps.
use std::collections::VecDeque;

pub struct GapAudio {
    samples: VecDeque<f32>,
    start: usize,
    last_emitted_end: Option<usize>,
    capacity: usize,
    max_gap: usize,
}

impl GapAudio {
    pub fn new(capacity: usize, max_gap: usize) -> Self {
        Self {
            samples: VecDeque::new(),
            start: 0,
            last_emitted_end: None,
            capacity,
            max_gap,
        }
    }

    pub fn append(&mut self, start: usize, samples: &[f32]) {
        if self.start + self.samples.len() != start {
            self.samples.clear();
            self.last_emitted_end = None;
            self.start = start;
        }
        self.samples.extend(samples.iter().copied());
        let excess = self.samples.len().saturating_sub(self.capacity);
        self.samples.drain(..excess);
        self.start += excess;
    }

    /// Returns the sample position after prepending available original audio.
    /// A missing range or long pause remains a separate audio span.
    pub fn prepend_gap(&mut self, start: usize, samples: &mut Vec<f32>) -> usize {
        let end = start + samples.len();
        let mut new_start = start;
        if let Some(previous_end) = self.last_emitted_end {
            if previous_end < start
                && start - previous_end <= self.max_gap
                && previous_end >= self.start
                && start <= self.start + self.samples.len()
            {
                let mut joined: Vec<_> = self
                    .samples
                    .range(previous_end - self.start..start - self.start)
                    .copied()
                    .collect();
                joined.append(samples);
                *samples = joined;
                new_start = previous_end;
            }
        }
        self.last_emitted_end = Some(end);
        new_start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_exact_gap_samples_once_and_preserves_end_position() {
        let mut history = GapAudio::new(32, 3);
        let pcm: Vec<_> = (0..20).map(|x| x as f32).collect();
        history.append(0, &pcm);
        let mut first = pcm[2..6].to_vec();
        assert_eq!(history.prepend_gap(2, &mut first), 2);
        let mut second = pcm[8..12].to_vec();
        assert_eq!(history.prepend_gap(8, &mut second), 6);
        assert_eq!(second, pcm[6..12]);
        let mut third = pcm[12..16].to_vec();
        assert_eq!(history.prepend_gap(12, &mut third), 12);
        assert_eq!(third, pcm[12..16]);
    }

    #[test]
    fn missing_old_audio_and_long_gaps_never_create_silence() {
        let mut history = GapAudio::new(8, 3);
        history.append(0, &[1.0; 20]);
        assert_eq!(history.samples.len(), 8);
        history.prepend_gap(3, &mut vec![2.0; 3]);
        let mut next = vec![3.0; 2];
        assert_eq!(history.prepend_gap(8, &mut next), 8);
        assert_eq!(next, vec![3.0; 2]);
        assert_eq!(history.prepend_gap(15, &mut next), 15);
    }

    #[test]
    fn discontinuous_input_resets_context() {
        let mut history = GapAudio::new(32, 3);
        history.append(0, &[1.0; 10]);
        history.prepend_gap(2, &mut vec![1.0; 3]);
        history.append(100, &[2.0; 10]);
        let mut next = vec![2.0; 3];
        assert_eq!(history.prepend_gap(102, &mut next), 102);
        assert_eq!(next.len(), 3);
    }
}
