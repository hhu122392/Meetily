//! Apply reviewed edits against the exact source version and character positions.

pub struct TextEdit<'a> {
    pub expected_text: &'a str,
    pub original: &'a str,
    pub suggested: &'a str,
    pub start_char: usize,
    pub end_char: usize,
}

pub fn apply_text_edits(text: &str, edits: &[TextEdit<'_>]) -> Result<String, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut ordered: Vec<&TextEdit<'_>> = edits.iter().collect();
    ordered.sort_by_key(|edit| (edit.start_char, edit.end_char));
    let mut last_end = 0;
    for (index, edit) in ordered.iter().enumerate() {
        if edit.expected_text != text {
            return Err("proofread_source_changed".into());
        }
        if edit.start_char >= edit.end_char || edit.end_char > chars.len()
            || edit.original == edit.suggested
            || chars[edit.start_char..edit.end_char].iter().collect::<String>() != edit.original
        {
            return Err("proofread_invalid_range".into());
        }
        if index > 0 && edit.start_char < last_end {
            return Err("proofread_overlapping_edits".into());
        }
        last_end = edit.end_char;
    }
    let mut result = String::new();
    let mut cursor = 0;
    for edit in ordered {
        result.extend(chars[cursor..edit.start_char].iter());
        result.push_str(edit.suggested);
        cursor = edit.end_char;
    }
    result.extend(chars[cursor..].iter());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_only_selected_occurrence() {
        let text = "店里的人正在讨论V店。";
        let edit = TextEdit { expected_text: text, original: "店", suggested: "4.1", start_char: 9, end_char: 10 };
        assert_eq!(apply_text_edits(text, &[edit]).unwrap(), "店里的人正在讨论V4.1。");
    }

    #[test]
    fn handles_unicode_and_multiple_original_positions() {
        let text = "🙂店和店";
        let edits = [
            TextEdit { expected_text: text, original: "店", suggested: "门店", start_char: 3, end_char: 4 },
            TextEdit { expected_text: text, original: "店", suggested: "商店", start_char: 1, end_char: 2 },
        ];
        assert_eq!(apply_text_edits(text, &edits).unwrap(), "🙂商店和门店");
    }

    #[test]
    fn rejects_stale_text_bad_ranges_and_overlaps() {
        let text = "小店";
        let edit = || TextEdit { expected_text: text, original: "店", suggested: "商场", start_char: 1, end_char: 2 };
        assert_eq!(apply_text_edits("大店", &[edit()]).unwrap_err(), "proofread_source_changed");
        assert_eq!(apply_text_edits(text, &[edit(), edit()]).unwrap_err(), "proofread_overlapping_edits");
        let mut bad = edit(); bad.start_char = 0;
        assert_eq!(apply_text_edits(text, &[bad]).unwrap_err(), "proofread_invalid_range");
    }
}
