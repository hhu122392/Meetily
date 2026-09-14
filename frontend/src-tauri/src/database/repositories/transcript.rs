use crate::api::{TranscriptSearchResult, TranscriptSegment};
use chrono::Utc;
use sqlx::{Connection, Error as SqlxError, SqlitePool};
use tracing::{error, info};
use uuid::Uuid;

pub struct TranscriptsRepository;

impl TranscriptsRepository {
    /// Saves a new meeting and its associated transcript segments.
    /// This function uses a transaction to ensure that either both the meeting
    /// and all its transcripts are saved, or none of them are.
    pub async fn save_transcript(
        pool: &SqlitePool,
        meeting_title: &str,
        transcripts: &[TranscriptSegment],
        folder_path: Option<String>,
    ) -> Result<String, SqlxError> {
        let meeting_id = format!("meeting-{}", Uuid::new_v4());

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let now = Utc::now();

        // 1. Create the new meeting
        let result = sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&meeting_id)
        .bind(meeting_title)
        .bind(now)
        .bind(now)
        .bind(&folder_path)
        .execute(&mut *transaction)
        .await;

        if let Err(e) = result {
            error!("Failed to create meeting '{}': {}", meeting_title, e);
            transaction.rollback().await?;
            return Err(e);
        }

        info!("Successfully created meeting with id: {}", meeting_id);

        // 2. Save each transcript segment with audio timing fields
        for segment in transcripts {
            let transcript_id = format!("transcript-{}", Uuid::new_v4());
            let result = sqlx::query(
                "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration)
                 VALUES (?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&transcript_id)
            .bind(&meeting_id)
            .bind(&segment.text)
            .bind(&segment.timestamp)
            .bind(segment.audio_start_time)
            .bind(segment.audio_end_time)
            .bind(segment.duration)
            .execute(&mut *transaction)
            .await;

            if let Err(e) = result {
                error!(
                    "Failed to save transcript segment for meeting {}: {}",
                    meeting_id, e
                );
                transaction.rollback().await?;
                return Err(e);
            }
        }

        info!(
            "Successfully saved {} transcript segments for meeting {}",
            transcripts.len(),
            meeting_id
        );

        // Commit the transaction
        transaction.commit().await?;

        Ok(meeting_id)
    }

    /// Searches for a query string within the transcripts.
    /// It returns a list of matching transcripts with context.
    pub async fn search_transcripts(
        pool: &SqlitePool,
        query: &str,
    ) -> Result<Vec<TranscriptSearchResult>, SqlxError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let search_query = format!("%{}%", query.to_lowercase());

        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT m.id, m.title, t.transcript, t.timestamp
             FROM meetings m
             JOIN transcripts t ON m.id = t.meeting_id
             WHERE LOWER(t.transcript) LIKE ?",
        )
        .bind(&search_query)
        .fetch_all(pool)
        .await?;

        let results = rows
            .into_iter()
            .map(|(id, title, transcript, timestamp)| {
                let match_context = Self::get_match_context(&transcript, query);
                TranscriptSearchResult {
                    id,
                    title,
                    match_context,
                    timestamp,
                }
            })
            .collect();

        Ok(results)
    }

    /// Helper function to extract a snippet of text around the first match of a query.
    ///
    /// P1-10：原实现用字节下标切 `&transcript[start..end]`。中文一个字符 3 字节，
    /// 命中位置减 100 字节几乎必然落在多字节字符中间，直接 panic；命令因此不返回，
    /// 界面永远停在「正在搜索…」且列表全空。这里改成按字符边界取窗口，
    /// 并逐字符记录大小写转换前后的位置，避免 `İ` 这类字符转换后长度变化时切错。
    fn get_match_context(transcript: &str, query: &str) -> String {
        const WINDOW_CHARS: usize = 100;

        let query_lower = query.to_lowercase();
        if query_lower.is_empty() {
            return transcript.chars().take(200).collect();
        }

        let mut lowered = String::new();
        let mut lowered_to_original: Vec<usize> = Vec::new();
        for (index, ch) in transcript.chars().enumerate() {
            for lowered_char in ch.to_lowercase() {
                lowered.push(lowered_char);
                lowered_to_original.push(index);
            }
        }

        let Some(byte_index) = lowered.find(&query_lower) else {
            return transcript.chars().take(200).collect(); // Fallback to the start of the transcript
        };

        let lowered_char_index = lowered[..byte_index].chars().count();
        let lowered_char_len = query_lower.chars().count();
        let total_chars = transcript.chars().count();

        let match_start_char = lowered_to_original
            .get(lowered_char_index)
            .copied()
            .unwrap_or(0);
        let match_end_char = lowered_to_original
            .get((lowered_char_index + lowered_char_len).saturating_sub(1))
            .copied()
            .unwrap_or(match_start_char);

        let start_char = match_start_char.saturating_sub(WINDOW_CHARS);
        let end_char = (match_end_char + 1 + WINDOW_CHARS).min(total_chars);

        let mut context = String::new();
        if start_char > 0 {
            context.push_str("...");
        }
        context.extend(
            transcript
                .chars()
                .skip(start_char)
                .take(end_char.saturating_sub(start_char)),
        );
        if end_char < total_chars {
            context.push_str("...");
        }
        context
    }
}

#[cfg(test)]
mod tests {
    use super::TranscriptsRepository;

    /// P1-10 回归：中文命中的上下文截取必须按字符边界，不能再 panic。
    #[test]
    fn match_context_is_char_boundary_safe_for_chinese() {
        let transcript = format!(
            "{}{}{}",
            "我们讨论了供应链和品牌定制的问题，".repeat(10),
            "中间这段是命中的供应链关键词，",
            "后面继续讨论别的话题。".repeat(10)
        );
        let context = TranscriptsRepository::get_match_context(&transcript, "供应链");
        assert!(context.contains("供应链"), "context should contain the match");

        let long = format!("{}供应链{}", "一".repeat(500), "二".repeat(500));
        let long_context = TranscriptsRepository::get_match_context(&long, "供应链");
        assert!(long_context.contains("供应链"));
        assert!(long_context.chars().count() <= 210);
    }
}
