"""Baseline smoke tests for AxPoint Parser Sidecar."""

from __future__ import annotations

import unittest

from parsers.engine import parse_bytes_to_markdown


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


if __name__ == "__main__":
    unittest.main()
