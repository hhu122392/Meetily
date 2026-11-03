CREATE TABLE IF NOT EXISTS company_workspace (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    company_id TEXT NOT NULL
);
