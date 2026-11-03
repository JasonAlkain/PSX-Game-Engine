#!/usr/bin/env python3
"""Checks the wiki: every page is listed in SUMMARY.md, and every relative
link or image points at a file that exists (and, for links to a page with a
#fragment, at a heading on that page). External links are not fetched.

Run from anywhere: python3 docs/wiki/check_links.py
"""

import os
import re
import sys

SRC = os.path.join(os.path.dirname(os.path.abspath(__file__)), "src")
LINK = re.compile(r"\]\(([^)\s]+)\)|src=\"([^\"]+)\"")
FENCE = re.compile(r"```.*?```", re.S)


def anchors(path):
    """mdBook heading ids: lower case, punctuation dropped, spaces to '-'."""
    ids = set()
    with open(path, encoding="utf-8") as f:
        text = FENCE.sub("", f.read())
    for line in text.splitlines():
        m = re.match(r"^#+\s+(.*)", line)
        if m:
            title = re.sub(r"[^\w\- ]", "", m.group(1).strip().lower())
            ids.add(title.replace(" ", "-"))
    return ids


def main():
    pages = []
    for root, _, files in os.walk(SRC):
        for name in files:
            if name.endswith(".md"):
                pages.append(os.path.relpath(os.path.join(root, name), SRC))
    with open(os.path.join(SRC, "SUMMARY.md"), encoding="utf-8") as f:
        summary = f.read()

    problems = []
    for page in sorted(pages):
        if page != "SUMMARY.md" and f"({page})" not in summary:
            problems.append(f"{page}: not listed in SUMMARY.md")
        path = os.path.join(SRC, page)
        with open(path, encoding="utf-8") as f:
            text = FENCE.sub("", f.read())
        for m in LINK.finditer(text):
            link = m.group(1) or m.group(2)
            if re.match(r"^[a-z]+:", link):
                continue
            target, _, fragment = link.partition("#")
            resolved = (
                os.path.normpath(os.path.join(os.path.dirname(path), target))
                if target
                else path
            )
            if not os.path.exists(resolved):
                problems.append(f"{page}: broken link {link}")
            elif fragment and resolved.endswith(".md") and fragment not in anchors(resolved):
                problems.append(f"{page}: no heading #{fragment} in {target or page}")

    for p in problems:
        print(p)
    print(f"{len(pages)} pages, {len(problems)} problems")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
