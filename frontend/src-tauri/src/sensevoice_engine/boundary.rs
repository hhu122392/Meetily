//! Join a re-decoded audio boundary by matching lexical tokens AND audio time.
//! These times are CTC emission positions, not exact word boundaries.

#[derive(Clone, Debug, PartialEq)]
pub struct TimedToken {
    pub text: String,
    pub time: f64,
}

#[derive(Clone, Debug)]
pub struct BoundaryResult {
    pub tokens: Vec<TimedToken>,
    pub status: &'static str,
}

pub fn join_boundary(
    previous: &[TimedToken],
    current: &[TimedToken],
    bridge: &[TimedToken],
    bridge_start: f64,
    bridge_end: f64,
    cut: f64,
    covers_previous_start: bool,
    covers_current_end: bool,
) -> BoundaryResult {
    let original = |status| BoundaryResult {
        tokens: previous.iter().chain(current).cloned().collect(),
        status,
    };
    if !valid_times(previous)
        || !valid_times(current)
        || !valid_times(bridge)
        || !bridge_start.is_finite()
        || !bridge_end.is_finite()
        || !cut.is_finite()
        || bridge_start > cut
        || cut > bridge_end
    {
        return original("invalid_timing");
    }
    if !bridge.iter().any(lexical) && previous.iter().chain(current).any(lexical) {
        return original("empty_bridge");
    }
    // Exclude the new decode's exposed edges and keep context on both sides
    // of the old cut. These are conservative guard values, not ASR guarantees.
    let left = anchors(previous, bridge, bridge_start + 3.0, cut - 0.5);
    // A short pending row can be covered in full by the original audio.
    // Only then may its beginning be replaced without a lexical anchor.
    let (old_start, new_start) = if covers_previous_start {
        (0, 0)
    } else {
        let Some(a) = left.last() else {
            return original("unmatched_left");
        };
        (a.old_start, a.new_start)
    };
    let mut tokens = previous[..old_start].to_vec();
    if covers_current_end {
        tokens.extend_from_slice(&bridge[new_start..]);
    } else {
        let right = anchors(current, bridge, cut + 0.5, bridge_end - 3.0);
        let Some(b) = right.first() else {
            return original("unmatched_right");
        };
        if new_start > b.new_end {
            return original("crossed_anchors");
        }
        tokens.extend_from_slice(&bridge[new_start..=b.new_end]);
        tokens.extend_from_slice(&current[b.old_end + 1..]);
    }
    if !valid_times(&tokens) {
        return original("nonmonotonic");
    }
    BoundaryResult {
        tokens,
        status: "bridged",
    }
}

pub fn valid_times(tokens: &[TimedToken]) -> bool {
    tokens.iter().all(|t| t.time.is_finite() && t.time >= 0.0)
        && tokens.windows(2).all(|t| t[0].time <= t[1].time)
}

fn lexical(token: &TimedToken) -> bool {
    token.text.chars().any(char::is_alphanumeric)
}

struct Anchor {
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
}

fn anchors(old: &[TimedToken], new: &[TimedToken], low: f64, high: f64) -> Vec<Anchor> {
    let old: Vec<_> = old.iter().enumerate().filter(|(_, t)| lexical(t)).collect();
    let new: Vec<_> = new.iter().enumerate().filter(|(_, t)| lexical(t)).collect();
    let mut found = Vec::new();
    for a in old.windows(4) {
        if a[0].1.time < low || a[3].1.time > high {
            continue;
        }
        let mut matches: Vec<_> = new
            .windows(4)
            .filter_map(|b| {
                a.iter()
                    .zip(b)
                    .all(|((_, x), (_, y))| x.text == y.text && (x.time - y.time).abs() <= 0.45)
                    .then(|| {
                        (
                            a.iter()
                                .zip(b)
                                .map(|((_, x), (_, y))| (x.time - y.time).abs())
                                .sum::<f64>(),
                            b,
                        )
                    })
            })
            .collect();
        matches.sort_by(|a, b| a.0.total_cmp(&b.0));
        let Some((score, b)) = matches.first() else {
            continue;
        };
        // Repeated phrases can match more than once. Do not choose arbitrarily
        // when the audio positions cannot distinguish those occurrences.
        if matches
            .get(1)
            .is_some_and(|other| (other.0 - score).abs() < 1e-6)
        {
            continue;
        }
        found.push(Anchor {
            old_start: a[0].0,
            old_end: a[3].0,
            new_start: b[0].0,
            new_end: b[3].0,
        });
    }
    found
}

pub fn token_text(tokens: &[TimedToken]) -> String {
    tokens
        .iter()
        .map(|t| t.text.as_str())
        .collect::<String>()
        .replace('▁', " ")
        .trim()
        .to_owned()
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
    fn keeps_repeated_speech_at_different_times() {
        let previous = tokens("这个方案可以可以先做。", 4.0);
        let current = tokens("可以可以继续推进。", 7.0);
        let bridge = [
            previous[..previous.len() - 1].to_vec(),
            tokens("，", 6.3),
            current.clone(),
        ]
        .concat();
        let result = join_boundary(&previous, &current, &bridge, 0.0, 14.0, 7.0, false, true);
        assert_eq!(result.status, "bridged");
        assert_eq!(token_text(&result.tokens).matches("可以").count(), 4);
    }

    #[test]
    fn same_words_elsewhere_cannot_anchor_a_join() {
        let previous = tokens("这里还有四个字。", 4.0);
        let current = tokens("后面继续讲话。", 10.0);
        let bridge = [tokens("这里还有四个字。", 1.0), current.clone()].concat();
        let result = join_boundary(&previous, &current, &bridge, 0.0, 18.0, 10.0, false, true);
        assert_eq!(result.status, "unmatched_left");
        assert_eq!(result.tokens, [previous, current].concat());
    }

    #[test]
    fn unmatched_right_keeps_original_audio_blocks() {
        let previous = tokens("这里还有四个字。", 4.0);
        let current = tokens("后面继续讲话。", 10.0);
        let bridge = [previous.clone(), tokens("不能用不同的文字接上。", 10.0)].concat();
        let result = join_boundary(&previous, &current, &bridge, 0.0, 22.0, 10.0, false, false);
        assert_eq!(result.status, "unmatched_right");
        assert_eq!(result.tokens, [previous, current].concat());
    }

    #[test]
    fn entire_short_utterance_can_be_redecoded_without_four_word_anchor() {
        let previous = tokens("几年前。", 1.0);
        let current = tokens("带着这些问题。", 4.0);
        let bridge = [tokens("几年前，", 1.0), current.clone()].concat();
        let result = join_boundary(&previous, &current, &bridge, 0.0, 8.0, 3.0, true, true);
        assert_eq!(result.status, "bridged");
        assert_eq!(token_text(&result.tokens), "几年前，带着这些问题。");
    }

    #[test]
    fn empty_bridge_cannot_erase_recognized_speech() {
        let previous = tokens("几年前。", 1.0);
        let current = tokens("带着这些问题。", 4.0);
        let result = join_boundary(&previous, &current, &[], 0.0, 8.0, 3.0, true, true);
        assert_eq!(result.status, "empty_bridge");
        assert_eq!(result.tokens, [previous, current].concat());
    }
}
