#!/usr/bin/env python3
"""Create a context from desktop ids and spoken words using only the stdlib."""
import argparse
import configparser
import json
import os
from pathlib import Path
import re
import subprocess
import sys


def installed_apps():
    directories = [Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))]
    directories.extend(Path(item) for item in os.environ.get(
        "XDG_DATA_DIRS", "/usr/local/share:/usr/share").split(os.pathsep) if item)
    apps = {}
    seen = set()
    for directory in directories:
        root = directory / "applications"
        for path in sorted(root.rglob("*.desktop")):
            desktop_id = str(path.relative_to(root)).replace(os.sep, "-")
            if desktop_id in seen:
                continue
            seen.add(desktop_id)
            config = configparser.ConfigParser(interpolation=None, strict=False)
            try:
                config.read(path, encoding="utf-8")
                entry = config["Desktop Entry"]
                if entry.getboolean("Hidden", fallback=False) or entry.getboolean("NoDisplay", fallback=False):
                    continue
                names = [entry.get("Name", desktop_id), entry.get("GenericName", "")]
                apps[desktop_id] = " / ".join(name for name in names if name)
            except (OSError, UnicodeError, configparser.Error, KeyError):
                continue
    return apps


def running_apps():
    try:
        result = subprocess.run(["ps", "-u", str(os.getuid()), "-o", "comm="],
                                capture_output=True, text=True, check=True)
        return set(result.stdout.splitlines())
    except (OSError, subprocess.CalledProcessError):
        return set()


def quote(value):
    return json.dumps(value, ensure_ascii=False)


def render(app_id, hotwords, apps):
    lines = [f"app_id = {quote(app_id)}", "", "[buttons]", "", "[hotwords]"]
    if not apps:
        lines.append("apps = false")
    seen = set()
    for spec in hotwords:
        word, separator, target = spec.partition("=")
        kind, colon, value = target.partition(":")
        if (not separator or not colon or kind not in {"app", "path", "action"}
                or not value.strip() or not re.fullmatch(r"[^\W_]+(?:\s+[^\W_]+)*", word, re.UNICODE)):
            raise ValueError("hotwords use WORD=app:ID, WORD=path:PATH or WORD=action:PHRASE")
        if word.lower() in seen or word == "apps":
            raise ValueError("hotword names must be unique; apps is reserved")
        seen.add(word.lower())
        if kind == "action":
            raise ValueError("add action references after defining [speech] in the generated context")
        if kind == "app" and (not value.endswith(".desktop") or "/" in value or value.startswith("-")):
            raise ValueError("app targets must be .desktop ids")
        lines.append(f"{quote(word)} = {{ {kind} = {quote(value)} }}")
    source = "\n".join(lines) + "\n"
    if len(source.encode()) > 65536:
        raise ValueError("context exceeds 64 KiB")
    return source


def ask(prompt):
    print(prompt, end="", file=sys.stderr, flush=True)
    return input()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name")
    parser.add_argument("--app-id", help="exact app match, or * for global hotwords")
    parser.add_argument("--hotword", action="append", default=[], metavar="WORD=app:ID|WORD=path:PATH")
    parser.add_argument("--no-apps", action="store_true", help="disable installed application hotwords")
    parser.add_argument("--contexts-dir", type=Path, default=Path(os.environ.get(
        "TEHUTI_CONTEXT_DIRECTORY", Path(os.environ.get(
            "XDG_CONFIG_HOME", Path.home() / ".config")) / "tehuti")),
        help="context directory passed to tehuti")
    parser.add_argument("--dry-run", action="store_true")
    options = parser.parse_args()
    try:
        name = options.name or ask("Context name: ").strip()
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]*", name):
            raise ValueError("name must contain letters, digits, underscores or hyphens")
        app_id = options.app_id
        if app_id is None:
            running = running_apps()
            for desktop_id, label in installed_apps().items():
                marker = " (possibly running)" if desktop_id.removesuffix(".desktop") in running else ""
                print(f"  {desktop_id}: {label}{marker}", file=sys.stderr)
            app_id = ask("Application id to match (or *): ").strip()
        if not app_id.strip() or any(ord(char) < 32 for char in app_id):
            raise ValueError("application id must not be blank or contain control characters")
        words = options.hotword
        if not words and sys.stdin.isatty():
            while True:
                word = ask("Hotword WORD=app:ID or WORD=path:PATH (blank to finish): ").strip()
                if not word:
                    break
                words.append(word)
        source = render(app_id, words, not options.no_apps)
        if options.dry_run:
            print(source, end="")
        else:
            destination = options.contexts_dir.expanduser() / name / "context.toml"
            destination.parent.mkdir(parents=True, exist_ok=True)
            with destination.open("x", encoding="utf-8") as output:
                output.write(source)
            print(destination)
    except (ValueError, OSError, EOFError) as error:
        parser.exit(2, f"{error}\n")


if __name__ == "__main__":
    main()
