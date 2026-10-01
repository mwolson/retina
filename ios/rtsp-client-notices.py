#!/usr/bin/env python3
"""Audit amp-rtsp-client's Rust dependencies and write its license notices.

Reads `cargo metadata` for amp-rtsp-client, fails if any locked crate lacks a
permissive license, and writes Retina-LICENSE.txt with the license texts of
every crate linked into the iOS static library (Retina itself comes from this
repository by path). It also writes RustStandardLibrary-LICENSE.txt from the
pinned toolchain's share/doc/rust/COPYRIGHT-library.html, and fails if the
active rustc is not the version amp-rtsp-client/rust-toolchain.toml pins.
build-xcframework.sh runs it for every build and ships both files in the
release zip:

    python3 rtsp-client-notices.py --out DIR [--audit AUDIT.md]

With --check DIR it compares existing notices in DIR with a fresh render
instead of writing them.

The app that links the xcframework must ship both files, since they are how it
meets its dependencies' attribution terms.
"""

import argparse
import html
import json
import re
import subprocess
import sys
from pathlib import Path

IOS_DIR = Path(__file__).resolve().parent
CRATE_DIR = IOS_DIR / "amp-rtsp-client"
NOTICES = "Retina-LICENSE.txt"
STD_NOTICES = "RustStandardLibrary-LICENSE.txt"
TARGET = "aarch64-apple-ios"
ALLOWED = {
    "0BSD",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "CC0-1.0",
    "ISC",
    "MIT",
    "MIT-0",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Unlicense",
    "Zlib",
}
LICENSE_FILE = re.compile(r"(?i)^(licen[sc]e|copying|copyright|notice|unlicense)")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--audit", type=Path, help="write a markdown audit of Cargo.lock here")
    outputs = parser.add_mutually_exclusive_group(required=True)
    outputs.add_argument("--out", type=Path, help="write the notices into this directory")
    outputs.add_argument("--check", type=Path, help="fail if the notices in this directory are stale")
    args = parser.parse_args()

    everything = metadata(None)
    linked = linked_packages(metadata(TARGET))
    root = everything["resolve"]["root"]
    problems = [p for p in everything["packages"] if p["id"] != root and not permissive(p["license"])]
    if args.audit:
        args.audit.write_text(audit(everything, linked))
    if problems:
        for package in problems:
            print(f"Not permissive: {package['name']} {package['version']} ({package['license']})", file=sys.stderr)
        sys.exit(1)
    rendered = {NOTICES: notices(linked), STD_NOTICES: std_notices(pinned_rustc())}
    if args.check:
        stale = [name for name, text in rendered.items() if read(args.check / name) != text]
        for name in stale:
            print(f"{args.check / name} is stale; rerun with --out", file=sys.stderr)
        sys.exit(1 if stale else 0)
    args.out.mkdir(parents=True, exist_ok=True)
    for name, text in rendered.items():
        (args.out / name).write_text(text)


def read(path):
    return path.read_text() if path.exists() else None


def metadata(target):
    command = ["cargo", "metadata", "--format-version", "1", "--locked", "--manifest-path", str(CRATE_DIR / "Cargo.toml")]
    if target:
        command += ["--filter-platform", target]
    return json.loads(subprocess.run(command, check=True, capture_output=True, text=True, cwd=CRATE_DIR).stdout)


def linked_packages(meta):
    """Crates compiled into the iOS static library (no proc macros or build scripts), except this one."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"]
    seen, stack = set(), [root]
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        for dep in nodes[current]["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]) and not is_proc_macro(packages[dep["pkg"]]):
                stack.append(dep["pkg"])
    return sorted((packages[i] for i in seen if i != root), key=lambda p: (p["name"], p["version"]))


def is_proc_macro(package):
    return any("proc-macro" in target["kind"] for target in package["targets"])


def permissive(expression):
    """True when an SPDX license expression lets us use the crate under allowlisted terms.

    OR needs one allowed side and AND needs both. A license with an exception
    counts only when the whole "LICENSE WITH EXCEPTION" pair is allowlisted.
    Cargo's old "MIT/Apache-2.0" form reads as OR. Anything that does not parse
    completely (unbalanced parentheses, a missing operand, two licenses with no
    operator, a name that is not an SPDX license or exception identifier) is not
    permissive, so the crate needs review.
    """
    try:
        return SpdxExpression(expression or "").evaluate()
    except ValueError:
        return False


class SpdxExpression:
    OPERATORS = {"AND", "OR", "WITH"}
    IDSTRING = r"[A-Za-z0-9.-]+"
    LICENSE = re.compile(rf"(?:DocumentRef-{IDSTRING}:)?LicenseRef-{IDSTRING}|(?!(?:DocumentRef|LicenseRef)-){IDSTRING}\+?")
    EXCEPTION = re.compile(rf"AdditionRef-{IDSTRING}|(?!AdditionRef-){IDSTRING}")

    def __init__(self, text):
        self.tokens = re.findall(r"\(|\)|[^\s()]+", text.replace("/", " OR "))
        self.position = 0

    def evaluate(self):
        result = self.parse_or()
        if self.position != len(self.tokens):
            raise ValueError(f"unexpected {self.tokens[self.position]!r}")
        return result

    def parse_or(self):
        results = [self.parse_and()]
        while self.accept("OR"):
            results.append(self.parse_and())
        return any(results)

    def parse_and(self):
        results = [self.parse_atom()]
        while self.accept("AND"):
            results.append(self.parse_atom())
        return all(results)

    def parse_atom(self):
        if self.accept("("):
            result = self.parse_or()
            if not self.accept(")"):
                raise ValueError("missing )")
            return result
        name = self.identifier(self.LICENSE, "license")
        if self.accept("WITH"):
            name = f"{name} WITH {self.identifier(self.EXCEPTION, 'exception')}"
        return name in ALLOWED

    def identifier(self, pattern, kind):
        if self.position == len(self.tokens):
            raise ValueError(f"expression ends before a {kind}")
        token = self.tokens[self.position]
        if token in self.OPERATORS or not pattern.fullmatch(token):
            raise ValueError(f"expected a {kind}, found {token!r}")
        self.position += 1
        return token

    def accept(self, token):
        if self.position < len(self.tokens) and self.tokens[self.position] == token:
            self.position += 1
            return True
        return False


def license_files(package):
    for directory in (Path(package["manifest_path"]).parent, CRATE_DIR / "licenses" / package["name"]):
        if directory.is_dir():
            files = sorted(f for f in directory.iterdir() if f.is_file() and LICENSE_FILE.match(f.name))
            if files:
                return files
    raise SystemExit(f"No license text for {package['name']} {package['version']}; add one under amp-rtsp-client/licenses/")


def notices(linked):
    lines = [
        "Retina RTSP client and its Rust dependencies",
        "",
        "Camera streams play through amp-rtsp-client, a small Rust library built on",
        "Retina (https://github.com/scottlamb/retina). The crates below are compiled",
        "into the app, each shown with its version, declared license and source. Their",
        "license texts follow, each printed once with the crates that ship it.",
        "",
    ]
    lines += [f"{p['name']} {p['version']} ({p['license']}) {p.get('repository') or ''}".rstrip() for p in linked]
    lines += [
        "",
        "The Rust standard library is also compiled into the app. Its notices are",
        "listed separately under Rust standard library.",
    ]
    texts = {}
    for package in linked:
        for path in license_files(package):
            text = path.read_text(encoding="utf-8", errors="replace").strip()
            entry = texts.setdefault(" ".join(text.split()), [text, []])
            name = f"{package['name']} {package['version']}"
            if name not in entry[1]:
                entry[1].append(name)
    for text, users in texts.values():
        lines += ["", "-" * 72, "Used by: " + ", ".join(users), "-" * 72, "", text]
    return "\n".join(lines) + "\n"


def pinned_rustc():
    """Returns the active toolchain's sysroot after checking it is the version rust-toolchain.toml pins."""
    pin = re.search(r'^channel\s*=\s*"([^"]+)"', (CRATE_DIR / "rust-toolchain.toml").read_text(), re.M)
    rustc = ["rustc", "--version"]
    version = subprocess.run(rustc, check=True, capture_output=True, text=True, cwd=CRATE_DIR).stdout.split()[1]
    if not pin or version != pin.group(1):
        raise SystemExit(f"rustc {version} is active but rust-toolchain.toml pins {pin and pin.group(1)}")
    sysroot = subprocess.run(
        ["rustc", "--print", "sysroot"], check=True, capture_output=True, text=True, cwd=CRATE_DIR
    ).stdout
    return version, Path(sysroot.strip()) / "share" / "doc" / "rust" / "COPYRIGHT-library.html"


def std_notices(toolchain):
    """Plain-text copy of the toolchain's standard library notices, license texts printed once."""
    version, path = toolchain
    page = path.read_text(encoding="utf-8")
    split = page.index('id="out-of-tree-dependencies"')
    lines = [
        f"Rust standard library (Rust {version})",
        "",
        "The Rust standard library is compiled into the app. It is dual-licensed under",
        "the Apache License 2.0 and the MIT license, copyright The Rust Project",
        "Developers (https://thanks.rust-lang.org). These notices are the toolchain's",
        "share/doc/rust/COPYRIGHT-library.html as text.",
        "",
        plain(page[page.index("</h2>", page.index('id="in-tree-files"')) + 5 : page.rindex("<h2", 0, split)]),
        "",
        "Dependencies of the standard library:",
        "",
    ]
    texts = {}
    for block in re.split(r"<h3>", page[split:])[1:]:
        name = plain(block[: block.index("</h3>")]).replace("\U0001f4e6", "").strip()
        fields = {k: plain(v) for k, v in re.findall(r"<p><b>([^<:]+):</b>(.*?)</p>", block, re.S)}
        lines.append(f"{name} ({fields.get('License', 'see notices')}) {fields.get('URL', '')}".rstrip())
        for body in re.findall(r"<pre>(.*?)</pre>", block, re.S):
            text = html.unescape(body).strip()
            entry = texts.setdefault(" ".join(text.split()), [text, []])
            if name not in entry[1]:
                entry[1].append(name)
    for text, users in texts.values():
        lines += ["", "-" * 72, "Used by: " + ", ".join(users), "-" * 72, "", text]
    return "\n".join(lines) + "\n"


def plain(fragment):
    text = re.sub(r"<(?:br|/p|/li|/h\d|/tr)\s*/?>", "\n", fragment)
    text = html.unescape(re.sub(r"<[^>]+>", "", text))
    return "\n".join(line.strip() for line in text.splitlines() if line.strip())


def audit(everything, linked):
    linked_ids = {p["id"] for p in linked}
    rows = []
    for package in sorted(everything["packages"], key=lambda p: (p["name"], p["version"])):
        if package["id"] == everything["resolve"]["root"]:
            continue
        if package["id"] in linked_ids:
            use = "linked into the app"
        elif is_proc_macro(package):
            use = "build time (proc macro)"
        else:
            use = "build time or other platforms"
        verdict = "permissive" if permissive(package["license"]) else "REVIEW"
        rows.append(f"| {package['name']} | {package['version']} | {package['license']} | {use} | {verdict} |")
    return "\n".join(
        [
            "# amp-rtsp-client Rust dependency license audit",
            "",
            f"Every dependency in `amp-rtsp-client/Cargo.lock` ({len(rows)} crates), from `cargo metadata`.",
            f"{len(linked)} are linked into the {TARGET} static library; the rest are proc macros,",
            "build-time helpers, or crates for other platforms that the lock file still lists.",
            "Allowed licenses: " + ", ".join(sorted(ALLOWED)) + ".",
            "",
            "| Crate | Version | License | Use | Verdict |",
            "| --- | --- | --- | --- | --- |",
            *rows,
            "",
        ]
    )


if __name__ == "__main__":
    main()
