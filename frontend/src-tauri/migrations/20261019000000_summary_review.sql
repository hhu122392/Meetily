CREATE TABLE summary_markdown_exports (
    meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
    path TEXT,
    error TEXT,
    updated_at TEXT NOT NULL
);

-- Old queued messages were never explicitly approved by a user.
UPDATE lark_app_deliveries
SET status='awaiting_confirmation', detail='纪要待审核，请预览并确认发送'
WHERE status='queued';
