#!/usr/bin/env python3
"""Refuses comments that cite where a decision came from instead of saying it.

A comment may not point at an agent's message ("Fugo 38436"), a QA row
("row 630"), a board item ("#451"), someone's ruling ("Fugo's rule 3") or Matt
("(Matt)"): none of them is in the repository, so no reader can check them,
and they go public with the source. A reason that matters goes in the code: a
name, a type, a test.

    scripts/check-comments.py               every tracked .rs and .dart file
    scripts/check-comments.py FILE...       only those
    scripts/check-comments.py --added REV   only lines added since REV (main)
"""

import re
import subprocess
import sys

CITATION = re.compile(
    r"\b[A-Z][a-z]+(?:'s)? (?!(?:19|20)\d\d\b)\d{4,6}\b"  # an agent's message: Fugo 38436
    r"|\b(?i:rows?) \d{2,}[a-z]?\b"  # a QA row: row 630
    r"|(?<![&\w\"])(?<!item )#\d{2,4}\b"  # a board item: #451
    r"|\b(?!Apple|Claktory|Google|Microsoft)[A-Z][a-z]+'s (?:rule|rules|ruling|rulings|pass|condition|conditions|sizing|gate|invariant|no-go|finding|findings|verdict|LOW|MEDIUM|HIGH)\b"  # Fugo's rule 3
    r"|\bmsgs? \d{3,}\b"  # a message number: msg 30760
    r"|\(Matt\)|\bMatt's (?:card|call|rule|decision|words)\b"
)
COMMENT = re.compile(r"^\s*//")
QUOTED = re.compile(r'"[^"]*"|`[^`]*`')
SKIP = ("vendor/", "/protocol/", "frb_generated", ".pb.dart", ".pbenum.dart", ".pbjson.dart", ".pbserver.dart")
SOURCE = re.compile(r"\.(rs|dart)$")


def tracked():
    listed = subprocess.run(["git", "ls-files", "*.rs", "*.dart"], capture_output=True, text=True, check=True)
    return [path for path in listed.stdout.split("\n") if path and not any(part in path for part in SKIP)]


def every_line(paths):
    for path in paths:
        with open(path, encoding="utf-8", errors="replace") as source:
            for number, line in enumerate(source, 1):
                yield path, number, line


def added_lines(rev):
    diff = subprocess.run(["git", "diff", "-U0", "--no-color", rev, "--", "*.rs", "*.dart"],
                          capture_output=True, text=True, check=True).stdout
    path, number = None, 0
    for line in diff.split("\n"):
        if line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else None
        elif line.startswith("@@"):
            number = int(re.search(r"\+(\d+)", line).group(1))
        elif line.startswith("+") and path and SOURCE.search(path) and not any(part in path for part in SKIP):
            yield path, number, line[1:]
            number += 1


def main(argv):
    if argv[:1] == ["--added"]:
        lines = added_lines(argv[1] if len(argv) > 1 else "main")
    else:
        lines = every_line(argv or tracked())
    found = 0
    for path, number, line in lines:
        if COMMENT.match(line) and CITATION.search(QUOTED.sub("", line)):
            found += 1
            print(f"{path}:{number}: {line.strip()}")
    if found:
        print(f"\n{found} comment lines cite an agent, a row, a board item or Matt. Say the reason in code instead.")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
