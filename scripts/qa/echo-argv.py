from __future__ import annotations

import json
import sys


print(json.dumps({"arguments": sys.argv[1:]}, ensure_ascii=True))
