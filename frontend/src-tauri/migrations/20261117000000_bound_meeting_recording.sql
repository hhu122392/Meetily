-- Existing group routes remain delivery-only until the owner binds again with local recording enabled.
ALTER TABLE lark_app_routes ADD COLUMN local_record_bound_at INTEGER;
