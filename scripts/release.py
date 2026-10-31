"""Explicit maintainer release: make release vX.Y.Z (Python 3.11+)."""
import re
import subprocess
import sys
import tomllib
from pathlib import Path


def run(*args, capture=False):
    print("+ " + " ".join(args), flush=True)
    return subprocess.run(args, check=True, text=True, stdout=subprocess.PIPE if capture else None)


def release(tag):
    if not re.fullmatch(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", tag):
        raise ValueError("Use a stable version tag such as v0.19.0")
    if run("git", "branch", "--show-current", capture=True).stdout.strip() != "main":
        raise ValueError("Release from main; switch branches yourself before retrying")
    if run("git", "tag", "--list", tag, capture=True).stdout.strip():
        raise ValueError(f"Local tag {tag} already exists; nothing changed")
    remote = subprocess.run(
        ["git", "ls-remote", "--exit-code", "--tags", "origin", f"refs/tags/{tag}"],
        check=False, text=True, stdout=subprocess.PIPE,
    )
    if remote.returncode == 0:
        raise ValueError(f"Remote tag {tag} already exists; nothing changed")
    if remote.returncode != 2:
        raise ValueError("Cannot check origin for existing tags; nothing changed")
    run("git", "fetch", "--no-tags", "origin", "main")
    run("git", "merge-base", "--is-ancestor", "FETCH_HEAD", "HEAD")

    manifest = Path("Cargo.toml")
    source = manifest.read_text()
    old = tomllib.loads(source)["package"]["version"]
    version = tag[1:]
    if tuple(map(int, version.split("."))) <= tuple(map(int, old.split("-", 1)[0].split("+", 1)[0].split("."))):
        raise ValueError(f"New version {version} must be greater than {old}")
    package = re.search(r"(?ms)^\[package\]\s*\n.*?(?=^\[|\Z)", source)
    if package is None:
        raise ValueError("Cannot find [package] in Cargo.toml")
    updated, count = re.subn(
        r'(?m)^(version\s*=\s*)"[^"\n]+"',
        lambda match: f'{match[1]}"{version}"', package[0],
    )
    if count != 1:
        raise ValueError("Expected one package version in Cargo.toml")
    manifest.write_text(source[:package.start()] + updated + source[package.end():])
    # Cargo updates the root package version in Cargo.lock before checking.
    # Failures intentionally leave the working tree intact for inspection.
    run("cargo", "check")
    run("git", "add", "--all")
    run("git", "commit", "-m", f"Release {tag}")
    run("git", "push", "origin", "HEAD:refs/heads/main")
    run("git", "tag", "-a", tag, "-m", f"Release {tag}")
    run("git", "push", "origin", f"refs/tags/{tag}")
    print(f"Published {tag}; GitHub's tag-triggered release workflow can now run.")


if __name__ == "__main__":
    try:
        if len(sys.argv) != 2:
            raise ValueError("Usage: make release vX.Y.Z")
        release(sys.argv[1])
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"Release stopped: {error}. No changes are automatically reverted.", file=sys.stderr)
        sys.exit(1)
