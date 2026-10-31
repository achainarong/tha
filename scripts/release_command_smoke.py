"""Offline release-command tests using disposable Git repositories and fake Cargo.

Run: python3 scripts/release_command_smoke.py
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReleaseCommand(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="tha-release-command-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.remote = self.root / "remote.git"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.env = {
            **os.environ,
            "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_TERMINAL_PROMPT": "0",
        }
        cargo = self.bin / "cargo"
        cargo.write_text(f"#!{sys.executable}\n" + '''import os,sys,tomllib
from pathlib import Path
assert sys.argv[1:] == ["check"]
if os.environ.get("FAIL_CHECK"): sys.exit(1)
version = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
Path("Cargo.lock").write_text('version = 4\\n[[package]]\\nname = "tha"\\nversion = "' + version + '"\\n')
''')
        cargo.chmod(0o700)
        self.git("init", "--bare", str(self.remote))
        self.git("init", "-b", "main")
        self.git("config", "user.name", "Release Test")
        self.git("config", "user.email", "release@example.invalid")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "tag.gpgsign", "false")
        (self.repo / "Cargo.toml").write_text('[package]\nname = "tha"\nversion = "0.18.0"\n')
        (self.repo / "Cargo.lock").write_text('version = 4\n[[package]]\nname = "tha"\nversion = "0.18.0"\n')
        shutil.copy(ROOT / "Makefile", self.repo)
        (self.repo / "scripts").mkdir()
        shutil.copy(ROOT / "scripts/release.py", self.repo / "scripts")
        self.git("add", "--all")
        self.git("commit", "-m", "Initial")
        self.git("remote", "add", "origin", str(self.remote))
        self.git("push", "origin", "main")
        self.initial = self.git("rev-parse", "HEAD")

    def command(self, *args, check=True):
        return subprocess.run(args, cwd=self.repo, env=self.env, check=check,
                              capture_output=True, text=True, timeout=20)

    def git(self, *args):
        return self.command("git", *args).stdout.strip()

    def release(self, version="v0.19.0"):
        return self.command("make", "release", version, check=False)

    def test_release_commits_all_files_and_pushes_matching_version_and_annotated_tag(self):
        (self.repo / "new-feature.txt").write_text("include untracked work\n")
        result = self.release()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.git("show", "HEAD:new-feature.txt"), "include untracked work")
        self.assertIn('version = "0.19.0"', self.git("show", "HEAD:Cargo.toml"))
        self.assertIn('version = "0.19.0"', self.git("show", "HEAD:Cargo.lock"))
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.git("cat-file", "-t", "v0.19.0"), "tag")
        head = self.git("rev-parse", "HEAD")
        self.assertEqual(self.git("--git-dir", str(self.remote), "rev-parse", "main"), head)
        self.assertEqual(self.git("--git-dir", str(self.remote), "rev-parse", "v0.19.0^{}"), head)

    def test_failed_cargo_check_never_commits_or_pushes(self):
        self.env["FAIL_CHECK"] = "1"
        self.assertNotEqual(self.release().returncode, 0)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.initial)
        self.assertEqual(self.git("--git-dir", str(self.remote), "rev-parse", "main"), self.initial)
        self.assertEqual(self.git("tag", "--list"), "")

    def test_wrong_branch_is_rejected_without_changing_version(self):
        self.git("switch", "-c", "feature")
        self.assertNotEqual(self.release().returncode, 0)
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_existing_remote_tag_is_rejected_without_changing_files(self):
        self.git("tag", "v0.19.0")
        self.git("push", "origin", "refs/tags/v0.19.0")
        self.git("tag", "-d", "v0.19.0")
        self.assertNotEqual(self.release().returncode, 0)
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_branch_behind_origin_is_rejected_without_changing_files(self):
        other = self.root / "other"
        self.git("clone", "--branch", "main", str(self.remote), str(other))
        for key, value in [("user.name", "Other"), ("user.email", "other@example.invalid"),
                           ("commit.gpgsign", "false")]:
            self.git("-C", str(other), "config", key, value)
        (other / "remote-change.txt").write_text("remote change\n")
        self.git("-C", str(other), "add", "--all")
        self.git("-C", str(other), "commit", "-m", "Remote change")
        self.git("-C", str(other), "push", "origin", "main")
        self.assertNotEqual(self.release().returncode, 0)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.git("rev-parse", "HEAD"), self.initial)

    def test_rejected_main_push_never_creates_or_pushes_tag(self):
        hook = self.remote / "hooks/pre-receive"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o700)
        self.assertNotEqual(self.release().returncode, 0)
        self.assertEqual(self.git("tag", "--list"), "")
        self.assertEqual(self.git("--git-dir", str(self.remote), "rev-parse", "main"), self.initial)

    def test_invalid_or_non_increasing_versions_are_rejected(self):
        for version in ["v0.18.0", "v0.17.0", "v01.19.0", "vnope"]:
            with self.subTest(version=version):
                self.assertNotEqual(self.release(version).returncode, 0)
                self.assertEqual(self.git("status", "--porcelain"), "")

    def test_plain_release_still_only_builds(self):
        result = self.command("make", "-n", "release")
        self.assertIn("cargo build --release --locked", result.stdout)
        self.assertNotIn("scripts/release.py", result.stdout)


if __name__ == "__main__":
    unittest.main()
