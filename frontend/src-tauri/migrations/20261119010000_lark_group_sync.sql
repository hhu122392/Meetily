-- 改为每个账号独立同步群指令，停用旧预约和待创建会议。
UPDATE lark_invitations SET status='cancelled',detail='已取消预约功能，请在即将入会时重新发送会议指令。' WHERE status IN ('queued','ready','failed');
DELETE FROM lark_app_routes WHERE invitation_id IS NOT NULL AND remote_id IS NULL;
DELETE FROM lark_app_routes WHERE remote_id IS NULL;
UPDATE lark_app_routes SET local_record_bound_at=NULL;
UPDATE lark_meeting_launches SET expires_at=0;
UPDATE lark_meeting_launches SET status='expired',detail='已停用群指令创建预约，请发送已有会议号或入会链接。' WHERE status IN ('queued','ready','link_failed');
CREATE TABLE lark_group_sync_cursors (
 app_id TEXT NOT NULL, owner_id TEXT NOT NULL, chat_id TEXT NOT NULL, through_time INTEGER NOT NULL,
 window_start INTEGER NOT NULL DEFAULT 0, window_end INTEGER NOT NULL DEFAULT 0, page_token TEXT NOT NULL DEFAULT '',
 PRIMARY KEY(app_id,owner_id,chat_id)
);
CREATE TABLE lark_group_commands (
 app_id TEXT NOT NULL, owner_id TEXT NOT NULL, message_id TEXT NOT NULL, chat_id TEXT NOT NULL,
 command_at INTEGER NOT NULL, meeting_no TEXT, detail TEXT NOT NULL,
 ack_state TEXT NOT NULL DEFAULT 'none',
 PRIMARY KEY(app_id,owner_id,message_id)
);
