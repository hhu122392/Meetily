//! One revisable tail; completed rows are never rewritten by later audio.
use super::boundary::{token_text, TimedToken};

#[derive(Clone, Debug)]
pub struct TranscriptDraft {
    pub row_id: u64,
    pub revision: u64,
    pub tokens: Vec<TimedToken>,
    pub audio_start: f64,
    pub audio_end: f64,
    pub is_partial: bool,
}

impl TranscriptDraft {
    pub fn text(&self) -> String {
        token_text(&self.tokens)
    }
}

#[derive(Clone, Default)]
pub struct TranscriptTail {
    pending: Option<TranscriptDraft>,
    next_id: u64,
}

impl TranscriptTail {
    pub fn pending(&self) -> Option<&TranscriptDraft> {
        self.pending.as_ref()
    }

    pub fn accept(
        &mut self,
        tokens: Vec<TimedToken>,
        cut: f64,
        end: f64,
        allow_split: bool,
    ) -> Vec<TranscriptDraft> {
        if !tokens.iter().any(lexical) {
            return Vec::new();
        }
        let previous = self.pending.take();
        let had_previous = previous.is_some();
        let mut row = match previous {
            Some(mut row) => {
                row.tokens = tokens;
                row.audio_end = end;
                row.revision += 1;
                row
            }
            None => {
                let row = TranscriptDraft {
                    row_id: self.next_id,
                    revision: 1,
                    tokens,
                    audio_start: cut,
                    audio_end: end,
                    is_partial: true,
                };
                self.next_id += 1;
                row
            }
        };
        if had_previous && allow_split {
            if let Some((split, time)) = sentence_split(&row.tokens, cut, end) {
                if time > row.audio_start && time < end {
                    let rest = row.tokens.split_off(split);
                    row.audio_end = time;
                    row.is_partial = false;
                    let tail = TranscriptDraft {
                        row_id: self.next_id,
                        revision: 1,
                        tokens: rest,
                        audio_start: time,
                        audio_end: end,
                        is_partial: true,
                    };
                    self.next_id += 1;
                    self.pending = Some(tail.clone());
                    return vec![row, tail];
                }
            }
        }
        self.pending = Some(row.clone());
        vec![row]
    }

    pub fn finish(&mut self) -> Vec<TranscriptDraft> {
        self.pending
            .take()
            .map(|mut row| {
                row.revision += 1;
                row.is_partial = false;
                row
            })
            .into_iter()
            .collect()
    }
}

fn lexical(token: &TimedToken) -> bool {
    token.text.chars().any(char::is_alphanumeric)
}

fn sentence_split(tokens: &[TimedToken], cut: f64, end: f64) -> Option<(usize, f64)> {
    let mut before = None;
    let mut after = None;
    for (i, token) in tokens.iter().enumerate() {
        if !matches!(token.text.as_str(), "。" | "！" | "？" | "." | "!" | "?") {
            continue;
        }
        let Some(left) = tokens[..i].iter().rfind(|t| lexical(t)) else {
            continue;
        };
        let Some(right_index) = (i + 1..tokens.len()).find(|&j| lexical(&tokens[j])) else {
            continue;
        };
        let right = &tokens[right_index];
        if token.text == "."
            && left.text.chars().all(|c| c.is_ascii_digit())
            && right.text.chars().all(|c| c.is_ascii_digit())
        {
            continue;
        }
        // A punctuation timestamp can lie in trailing silence. Use its
        // neighbouring spoken tokens for an estimated playback boundary.
        let boundary = ((left.time + right.time) / 2.0).max(tokens[right_index - 1].time);
        if left.time <= cut {
            before = Some((right_index, boundary));
        } else if after.is_none() && right.time < end - 1.0 {
            after = Some((right_index, boundary));
        }
    }
    before.or(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tokens(text: &str, start: f64) -> Vec<TimedToken> {
        text.chars()
            .enumerate()
            .map(|(i, c)| TimedToken {
                text: c.to_string(),
                time: start + i as f64 * 0.2,
            })
            .collect()
    }

    #[test]
    fn next_audio_revises_tail_and_moves_the_whole_year_into_next_row() {
        let mut tail = TranscriptTail::default();
        let first = tail.accept(tokens("牛顿第三运动定律。自20世纪。", 0.0), 0.0, 3.0, true);
        assert_eq!(first.len(), 1);
        assert!(first[0].is_partial);
        let updates = tail.accept(
            tokens("牛顿第三运动定律。自20世纪60年代以来。", 0.0),
            3.0,
            5.0,
            true,
        );
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].row_id, first[0].row_id);
        assert_eq!(updates[0].revision, 2);
        assert!(!updates[0].is_partial);
        assert_eq!(updates[0].text(), "牛顿第三运动定律。");
        assert_eq!(updates[1].text(), "自20世纪60年代以来。");
        assert!(updates[1].is_partial);
        assert_eq!(updates[0].audio_end, updates[1].audio_start);
        assert!(updates[0].audio_end < 3.0);
    }

    #[test]
    fn long_unfinished_sentence_does_not_become_a_new_row_at_every_audio_cut() {
        let mut tail = TranscriptTail::default();
        tail.accept(tokens("这个方案", 0.0), 0.0, 1.0, true);
        let second = tail.accept(tokens("这个方案还是需要继续讨论", 0.0), 1.0, 3.0, true);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].row_id, 0);
        assert_eq!(second[0].audio_start, 0.0);
        assert_eq!(second[0].revision, 2);
        assert_eq!(second[0].text(), "这个方案还是需要继续讨论");
        assert!(second[0].is_partial);
    }

    #[test]
    fn stop_finalizes_the_same_tail_once_without_adding_or_deleting_words() {
        let mut tail = TranscriptTail::default();
        tail.accept(tokens("这个这个方案还", 0.0), 0.0, 2.0, true);
        let done = tail.finish();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].row_id, 0);
        assert_eq!(done[0].revision, 2);
        assert_eq!(done[0].text(), "这个这个方案还");
        assert!(!done[0].is_partial);
        assert!(tail.finish().is_empty());
    }

    #[test]
    fn a_decimal_point_does_not_end_a_sentence() {
        let mut tail = TranscriptTail::default();
        tail.accept(tokens("增加3.5", 0.0), 0.0, 1.0, true);
        let updates = tail.accept(tokens("增加3.5亿元用于研发", 0.0), 1.0, 3.0, true);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].row_id, 0);
    }

    #[test]
    fn estimated_row_end_includes_trailing_punctuation_time() {
        let mut tail = TranscriptTail::default();
        let first = vec![
            TimedToken {
                text: "好".into(),
                time: 1.0,
            },
            TimedToken {
                text: "。".into(),
                time: 3.5,
            },
        ];
        tail.accept(first.clone(), 0.0, 3.6, false);
        let joined = [
            first,
            vec![TimedToken {
                text: "继续".into(),
                time: 4.0,
            }],
        ]
        .concat();
        let updates = tail.accept(joined, 3.6, 6.0, true);
        assert_eq!(updates.len(), 2);
        assert!(updates[0]
            .tokens
            .iter()
            .all(|token| token.time <= updates[0].audio_end));
        assert!(updates[1]
            .tokens
            .iter()
            .all(|token| token.time >= updates[1].audio_start));
    }
}
