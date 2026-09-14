import json
import sqlite3
import sys

database_path = sys.argv[1]
connection = sqlite3.connect(f"file:{database_path}?mode=ro", uri=True)
tables = [
    row[0]
    for row in connection.execute(
        "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name"
    )
]
print(
    json.dumps(
        {
            table: [dict(zip(["cid", "name", "type", "notnull", "default", "pk"], row))
                    for row in connection.execute(f"PRAGMA table_info({table})")]
            for table in tables
        },
        ensure_ascii=False,
        indent=2,
    )
)
