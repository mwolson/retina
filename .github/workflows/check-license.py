#!/usr/bin/env python3
# Copyright (C) The Retina Authors
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Checks that expected header lines are present.

Call in either of two modes:

has-license.py FILE [...]
    check if all files with certain extensions have expected lines.
    This is useful in a CI action.

has-license.py
    check if stdin has expected lines.
    This is useful in a pre-commit hook, as in
    git-format-staged --no-write --formatter '.../has-license.py' '*.rs'
"""
import os
import re
import sys

# Filenames matching this regexp are expected to have the header lines.
FILENAME_MATCHER = re.compile(r'.*\.rs$')

MAX_LINE_COUNT = 10

EXPECTED_LINES = [
  re.compile(r'Copyright \(C\) The Retina Authors'),
  re.compile(r'SPDX-License-Identifier: MIT OR Apache-2\.0'),
]

# Fork addition: the AMP iOS wrapper under ios/ has its own copyright holder.
IOS_EXPECTED_LINES = [re.compile(r'Copyright \(C\) Mike Olson'), EXPECTED_LINES[1]]

def has_license(f, expected_lines=EXPECTED_LINES):
  """Returns if all of expected_lines are present within the first
  MAX_LINE_COUNT lines of f."""
  needed = set(expected_lines)
  i = 0
  for line in f:
    if i == 10:
      break
    i += 1
    for e in needed:
      if e.search(line):
        needed.remove(e)
        break
    if not needed:
      return True
  return False


def file_has_license(filename):
  in_ios = os.path.normpath(filename).split(os.sep)[0] == 'ios'
  with open(filename, 'r') as f:
    return has_license(f, IOS_EXPECTED_LINES if in_ios else EXPECTED_LINES)


def main(args):
  if not args:
    sys.exit(0 if has_license(sys.stdin) else 1)

  missing = [f for f in args
             if FILENAME_MATCHER.match(f) and not file_has_license(f)]
  if missing:
    print('The following files are missing expected copyright/license headers:', file=sys.stderr)
    print('\n'.join(missing), file=sys.stderr)
    sys.exit(1)


if __name__ == '__main__':
  main(sys.argv[1:])
