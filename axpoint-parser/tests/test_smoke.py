from __future__ import annotations

import os
import unittest
from unittest.mock import patch

from fastapi.testclient import TestClient

from main import app
from parsers.engine import parse_bytes_to_markdown

SMOKE_INTERNAL_TOKEN = "axp-smoke-internal-token-0123456789abcdef"


class ParserSmokeTest(unittest.TestCase):
    def test_plain_markdown_and_csv(self) -> None:
        title, md, engine = parse_bytes_to_markdown("sample.md", b"# Hello\n\nWorld")
        self.assertEqual(title, "sample")
        self.assertEqual(engine, "plain-utf8")
        self.assertIn("World", md)

        title2, csv_text, engine2 = parse_bytes_to_markdown("data.csv", b"a,b\n1,2\n")
        self.assertEqual(title2, "data")
        self.assertEqual(engine2, "plain-utf8")
        self.assertIn("a,b", csv_text)

    def test_sidecar_http_smoke_with_internal_token_middleware(self) -> None:
        with patch.dict(os.environ, {"AXPOINT_INTERNAL_TOKEN": SMOKE_INTERNAL_TOKEN}):
            client = TestClient(app)

            health_resp = client.get("/health")
            self.assertEqual(health_resp.status_code, 200)
            self.assertEqual(health_resp.json()["status"], "ok")

            unauth_resp = client.post(
                "/parse",
                files={"file": ("sample.md", b"# Smoke\n\nBody", "text/markdown")},
            )
            self.assertEqual(unauth_resp.status_code, 401)
            self.assertEqual(unauth_resp.json()["error_code"], "TOKEN_INVALID")

            auth_resp = client.post(
                "/parse",
                files={"file": ("sample.md", b"# Smoke\n\nBody", "text/markdown")},
                headers={"x-axpoint-internal-token": SMOKE_INTERNAL_TOKEN},
            )
            self.assertEqual(auth_resp.status_code, 200)
            payload = auth_resp.json()
            self.assertEqual(payload["title"], "sample")
            self.assertEqual(payload["engine"], "plain-utf8")
            self.assertIn("Smoke", payload["markdown"])


if __name__ == "__main__":
    unittest.main()
