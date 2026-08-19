from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "verify-doc-i18n.py"


class VerifyDocI18nTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        subprocess.run(["git", "init", "-q"], cwd=self.root, check=True)
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["crates/demo"]\n', encoding="utf-8"
        )
        crate = self.root / "crates/demo"
        (crate / "docs").mkdir(parents=True)
        (crate / "Cargo.toml").write_text(
            '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2024"\n',
            encoding="utf-8",
        )
        self.write_pair(crate / "README.md")

    def tearDown(self) -> None:
        self.temp.cleanup()

    def write_pair(self, source: Path) -> None:
        source.write_text(
            "# Demo\n\nEnglish | [中文](README.zh.md)\n\n## Use\n\n- Run `demo`.\n",
            encoding="utf-8",
        )
        source.with_name("README.zh.md").write_text(
            "# Demo\n\n[English](README.md) | 中文\n\n## 使用\n\n- 运行 `demo`。\n",
            encoding="utf-8",
        )

    def run_script(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(self.root), *args],
            cwd=self.root,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

    def test_recorded_pair_passes_and_one_sided_edit_fails(self) -> None:
        source = "crates/demo/README.md"
        self.assertEqual(self.run_script("--write", source).returncode, 0)
        self.assertEqual(self.run_script(source).returncode, 0)
        path = self.root / source
        path.write_text(path.read_text(encoding="utf-8").replace("Run", "Start"), encoding="utf-8")
        result = self.run_script(source)
        self.assertEqual(result.returncode, 1)
        self.assertIn("out of sync", result.stderr)

    def test_missing_counterpart_fails(self) -> None:
        (self.root / "crates/demo/README.zh.md").unlink()
        result = self.run_script("crates/demo/README.md")
        self.assertEqual(result.returncode, 1)
        self.assertIn("incomplete bilingual pair", result.stderr)

    def test_cached_mode_checks_the_staged_pair(self) -> None:
        source = "crates/demo/README.md"
        self.assertEqual(self.run_script("--write", source).returncode, 0)
        subprocess.run(["git", "add", "."], cwd=self.root, check=True)
        self.assertEqual(self.run_script("--cached", source).returncode, 0)

        path = self.root / source
        path.write_text(path.read_text(encoding="utf-8").replace("Run", "Start"), encoding="utf-8")
        self.assertEqual(self.run_script("--cached", source).returncode, 0)
        subprocess.run(["git", "add", source], cwd=self.root, check=True)
        result = self.run_script("--cached", source)
        self.assertEqual(result.returncode, 1)
        self.assertIn("out of sync", result.stderr)

    def test_cross_crate_link_cannot_be_recorded(self) -> None:
        source = self.root / "crates/demo/README.md"
        zh = self.root / "crates/demo/README.zh.md"
        source.write_text(source.read_text(encoding="utf-8") + "\n[Other](../other/README.md)\n", encoding="utf-8")
        zh.write_text(zh.read_text(encoding="utf-8") + "\n[Other](../other/README.md)\n", encoding="utf-8")
        result = self.run_script("--write", "crates/demo/README.md")
        self.assertEqual(result.returncode, 1)
        self.assertIn("escapes crate root", result.stderr)

    def test_agent_document_cannot_link_to_repository_root(self) -> None:
        path = self.root / "crates/demo/AGENTS.md"
        path.write_text("# Rules\n\n[Root](../../AGENTS.md)\n", encoding="utf-8")
        result = self.run_script("crates/demo/AGENTS.md")
        self.assertEqual(result.returncode, 1)
        self.assertIn("escapes crate root", result.stderr)

    def test_fenced_markdown_example_is_not_a_document_link(self) -> None:
        path = self.root / "crates/demo/AGENTS.md"
        path.write_text(
            "# Rules\n\n```markdown\n[Example](../../AGENTS.md)\n```\n",
            encoding="utf-8",
        )
        self.assertEqual(self.run_script("crates/demo/AGENTS.md").returncode, 0)

    def test_structure_mismatch_cannot_be_recorded(self) -> None:
        path = self.root / "crates/demo/README.zh.md"
        path.write_text(path.read_text(encoding="utf-8") + "\n### Extra\n", encoding="utf-8")
        result = self.run_script("--write", "crates/demo/README.md")
        self.assertEqual(result.returncode, 1)
        self.assertIn("headings structure differs", result.stderr)


if __name__ == "__main__":
    unittest.main()
