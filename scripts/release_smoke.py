"""Offline release contracts. No network, credentials, tags, or live publishing.

Run: uv run --with pyyaml python scripts/release_smoke.py
Validate a built package: python3 scripts/release_smoke.py --archive dist/ASSET
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ("tha-linux-amd64.tar.gz", "tha-linux-arm64.tar.gz", "tha-macos-arm64.tar.gz")


def check_archive(path):
    with tarfile.open(path, "r:gz") as archive:
        members = archive.getmembers()
        if sorted(member.name for member in members) != ["LICENSE", "README.md", "tha"]:
            raise ValueError("Archive must contain exactly tha, LICENSE, README.md (no duplicate or hidden files)")
        if any(not member.isfile() for member in members):
            raise ValueError("Archive entries must be regular files; links and devices are forbidden")
        binary = archive.getmember("tha")
        if not binary.mode & 0o111 or binary.size == 0:
            raise ValueError("Packaged tha must be nonempty and executable")


def fixture_archive(path, members=None):
    members = members or {"tha": b"#!/bin/sh\necho 'tha fixture'\n", "LICENSE": b"MIT\n", "README.md": b"fixture\n"}
    with tarfile.open(path, "w:gz") as archive:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = 0o755 if name == "tha" else 0o644
            archive.addfile(info, io.BytesIO(data))


class Contracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        import yaml
        cls.workflow = yaml.load((ROOT / ".github/workflows/release.yml").read_text(), Loader=yaml.BaseLoader)
        cls.installer = (ROOT / "README.md").read_text().split("```sh\n", 1)[1].split("```", 1)[0]

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="tha-release-contract-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.bin = self.directory / "bin"
        self.bin.mkdir()
        # No inherited HOME/config/token/socket access. Every invoked network tool is a fixture.
        self.env = {"PATH": str(self.bin) + os.pathsep + os.defpath, "HOME": str(self.directory / "home"),
                    "TMPDIR": str(self.directory), "PYTHON": sys.executable}
        self.program("curl", "raise SystemExit('unexpected curl')")
        self.program("gh", "raise SystemExit('unexpected gh')")

    def program(self, name, code):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n" + code + "\n")
        path.chmod(0o700)

    def step(self, job, name):
        return next(step["run"] for step in self.workflow["jobs"][job]["steps"] if step.get("name") == name)

    def run_shell(self, script, **environment):
        return subprocess.run(["/bin/sh", "-eu", "-c", script], cwd=self.directory,
                              env={**self.env, **environment}, capture_output=True, text=True, timeout=20)

    def test_installer_matrix_and_preserves_previous_install_on_all_verification_failures(self):
        fixtures = self.directory / "fixtures"
        fixtures.mkdir()
        for asset in ASSETS:
            fixture_archive(fixtures / asset)
        valid = "".join(f"{hashlib.sha256((fixtures / asset).read_bytes()).hexdigest()}  {asset}\n" for asset in ASSETS)
        self.program("uname", "import os,sys\nprint(os.environ['OS'] if sys.argv[1]=='-s' else os.environ['MACHINE'])")
        self.program("curl", """import os,sys,shutil
from pathlib import Path
args=sys.argv[1:]
assert len(args)==7 and args[:4] == ['--proto', '=https', '--tlsv1.2', '-fsSL'] and args[5]=='-o'
url=args[4]
assert url.startswith('https://github.com/achainarong/tha/releases/latest/download/')
if os.environ.get('FAIL_DOWNLOAD') == '1': raise SystemExit(22)
shutil.copyfile(Path(os.environ['FIXTURES']) / url.rsplit('/',1)[1], args[args.index('-o')+1])
""")
        destination = Path(self.env["HOME"]) / ".local/bin/tha"
        destination.parent.mkdir(parents=True)
        cases = [("Linux", "x86_64", "ok"), ("Linux", "aarch64", "ok"), ("Darwin", "arm64", "ok"),
                 ("Linux", "amd64", "ok"), ("Linux", "arm64", "ok"), ("Darwin", "x86_64", "unsupported"),
                 ("Linux", "armv7l", "unsupported"), ("Linux", "x86_64", "missing"),
                 ("Linux", "x86_64", "duplicate"), ("Linux", "x86_64", "mismatch"),
                 ("Linux", "x86_64", "download"), ("Linux", "x86_64", "broken-tar")]
        for system, machine, mode in cases:
            with self.subTest(system=system, machine=machine, mode=mode):
                manifest = valid
                if mode == "missing": manifest = ""
                if mode == "duplicate": manifest += valid
                if mode == "mismatch": manifest = manifest.replace(manifest[:64], "0" * 64)
                archive = fixtures / ASSETS[0]
                original = archive.read_bytes()
                if mode == "broken-tar":
                    archive.write_bytes(b"not a tar file")
                    manifest = f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {ASSETS[0]}\n"
                (fixtures / "SHA256SUMS").write_text(manifest)
                destination.write_text("previous-install")
                result = self.run_shell(self.installer, OS=system, MACHINE=machine, FIXTURES=str(fixtures),
                                        FAIL_DOWNLOAD="1" if mode == "download" else "0")
                archive.write_bytes(original)
                if mode == "ok":
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(destination.read_bytes(), b"#!/bin/sh\necho 'tha fixture'\n")
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(destination.read_text(), "previous-install")
                self.assertEqual(list(self.directory.glob("tha-install.*")), [])

    def test_version_step_matches_tag_and_classifies_prereleases(self):
        script = self.step("validate", "Require the tag to match Cargo.toml")
        self.program("git", "print('fixture-commit')")
        # Workflow uses python3, resolved from a fixed interpreter rather than host configuration.
        (self.bin / "python3").symlink_to(sys.executable)
        for version, tag, expected in [("0.1.0", "v0.1.0", "false"), ("0.2.0-rc.1", "v0.2.0-rc.1", "true"),
                                        ("0.2.0+build-meta", "v0.2.0+build-meta", "false"), ("0.1.0", "v9.0.0", None)]:
            with self.subTest(version=version, tag=tag):
                (self.directory / "Cargo.toml").write_text(f'[package]\nversion = "{version}"\n')
                output = self.directory / "output"
                output.write_text("")
                result = self.run_shell(script, GITHUB_REF_NAME=tag, GITHUB_OUTPUT=str(output))
                self.assertEqual(result.returncode == 0, expected is not None, result.stderr)
                if expected is not None:
                    self.assertIn(f"prerelease={expected}\n", output.read_text())
                else:
                    self.assertEqual(output.read_text(), "")

    def test_asset_manifest_rejects_missing_and_extra_files(self):
        script = self.step("publish", "Validate complete asset set and write SHA256SUMS")
        (self.bin / "python3").symlink_to(sys.executable)
        self.program("sha256sum", """import hashlib,sys
from pathlib import Path
assert sys.argv[1:]==['--check','SHA256SUMS']
for line in Path('SHA256SUMS').read_text().splitlines():
    digest,name=line.split()
    assert digest==hashlib.sha256(Path(name).read_bytes()).hexdigest()
""")
        dist = self.directory / "dist"
        dist.mkdir()
        for asset in ASSETS: fixture_archive(dist / asset)
        result = self.run_shell(script)
        self.assertEqual(result.returncode, 0, result.stderr)
        for line in (dist / "SHA256SUMS").read_text().splitlines():
            digest, name = line.split()
            self.assertEqual(digest, hashlib.sha256((dist / name).read_bytes()).hexdigest())
        (dist / "SHA256SUMS").unlink()
        (dist / ASSETS[0]).unlink()
        self.assertNotEqual(self.run_shell(script).returncode, 0)
        fixture_archive(dist / ASSETS[0])
        (dist / "unexpected").write_text("no")
        self.assertNotEqual(self.run_shell(script).returncode, 0)

    def test_publish_only_after_upload_and_never_overwrites_or_uses_moved_tags(self):
        script = self.step("publish", "Publish completed draft release")
        dist = self.directory / "dist"
        dist.mkdir()
        for asset in ASSETS: fixture_archive(dist / asset)
        (dist / "SHA256SUMS").write_text("fixture")
        self.program("gh", """import json,os,sys
from pathlib import Path
args=sys.argv[1:]
with open('calls.jsonl','a') as log: log.write(json.dumps(args)+'\\n')
mode=os.environ['MODE']
if args[0]=='api': print('moved' if mode=='moved' else 'fixture-commit')
elif args[:2]==['release','view']: raise SystemExit(0 if mode=='exists' else 1)
elif args[:2]==['release','create']:
    assert '--draft' in args and '--verify-tag' in args
    assert sorted(arg for arg in args if arg.startswith('dist/')) == sorted('dist/'+name for name in ['tha-linux-amd64.tar.gz','tha-linux-arm64.tar.gz','tha-macos-arm64.tar.gz','SHA256SUMS'])
    if mode=='upload-failure': raise SystemExit(1)
elif args[:2]==['release','edit']:
    assert args[-1]=='--draft=false'
    if mode=='edit-failure': raise SystemExit(1)
else: raise SystemExit('unexpected command')
""")
        for mode in ["ok", "moved", "exists", "upload-failure", "edit-failure"]:
            with self.subTest(mode=mode):
                calls = self.directory / "calls.jsonl"
                calls.write_text("")
                result = self.run_shell(script, GH_REPO="fixture/tha", TAG="v0.1.0", PRERELEASE="false",
                                        EXPECTED_COMMIT="fixture-commit", MODE=mode)
                self.assertEqual(result.returncode == 0, mode == "ok", result.stderr)
                operations = [json.loads(line)[:2] for line in calls.read_text().splitlines()]
                if mode in ["moved", "exists"]: self.assertNotIn(["release", "create"], operations)
                if mode in ["moved", "exists", "upload-failure"]: self.assertNotIn(["release", "edit"], operations)
                self.assertNotIn(["release", "delete"], operations)

    def test_package_structure_and_native_release_matrix(self):
        archive = self.directory / "fixture.tar.gz"
        fixture_archive(archive)
        check_archive(archive)
        for members in [{"tha": b"x", "../README.md": b"x", "LICENSE": b"x"},
                        {"tha": b"x", "README.md": b"x", "LICENSE": b"x", "._tha": b"x"}]:
            fixture_archive(archive, members)
            with self.assertRaises(ValueError): check_archive(archive)
        with tarfile.open(archive, 'w:gz') as package:
            for name in ['tha', 'LICENSE', 'README.md']:
                member = tarfile.TarInfo(name)
                member.type = tarfile.SYMTYPE
                member.linkname = '/outside-the-package'
                package.addfile(member)
        with self.assertRaises(ValueError): check_archive(archive)
        matrix = self.workflow["jobs"]["build"]["strategy"]["matrix"]["include"]
        self.assertEqual({entry["asset"] for entry in matrix}, set(ASSETS))
        self.assertEqual({entry["target"] for entry in matrix}, {"x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl", "aarch64-apple-darwin"})
        self.assertEqual(self.workflow["jobs"]["publish"]["needs"], ["validate", "build"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, help="Verify a produced archive without extracting or executing it")
    options = parser.parse_args()
    if options.archive:
        check_archive(options.archive)
        print(f"PASS: exact regular-file archive structure: {options.archive}")
    else:
        unittest.main(argv=[sys.argv[0]], verbosity=2)
