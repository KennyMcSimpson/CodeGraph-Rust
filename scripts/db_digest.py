#!/usr/bin/env python3
"""Content digest of a `cgr export --sqlite` file, to compare two indexes row for row.

    scripts/db_digest.py graph.sqlite            # one line per table plus the total
    scripts/db_digest.py a.sqlite b.sqlite       # also says whether they are identical

Rows are read in primary-key order, so node ids, edge order and reference order all take part:
two runs that print the same digest produced the same snapshot content. The `meta` table (which
holds the generation and the project path) is not part of the digest.
"""
import hashlib
import sqlite3
import sys

TABLES = ["files", "nodes", "edges", "refs", "imports"]


def digest(path):
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    out, total = {}, hashlib.sha256()
    for t in TABLES:
        h = hashlib.sha256()
        n = 0
        for row in db.execute(f"SELECT * FROM {t} ORDER BY rowid"):
            h.update(repr(row).encode())
            n += 1
        out[t] = (n, h.hexdigest()[:16])
        total.update(h.digest())
    return out, total.hexdigest()[:16]


def main(argv):
    if not 1 <= len(argv) <= 2:
        print(__doc__)
        return 2
    results = [digest(p) for p in argv]
    for p, (tables, total) in zip(argv, results):
        print(p)
        for t, (n, d) in tables.items():
            print(f"  {t:<8} {n:>9} rows  {d}")
        print(f"  total    {total}")
    if len(results) == 2:
        same = results[0][1] == results[1][1]
        print("IDENTICAL" if same else "DIFFERENT")
        return 0 if same else 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
