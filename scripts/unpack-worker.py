#!/usr/bin/env python3
"""Unpacks a Worker built in another job into a checkout, for a deploy job
holding the environment's secrets.

  unpack-worker.py <download directory> <checkout>

The build ran code that may be nobody's reviewed work, so the download is what
that code chose to write. The directory must hold exactly one regular file,
worker.tar: a download holding anything else could seed what a later step reads.
In the archive only plain files and directories under the three places a
Worker's build lives are taken, within MAX_MEMBERS and MAX_BYTES. Any other
member refuses the whole archive before anything is changed: a path into the
checkout's scripts or node_modules, `..`, an absolute path, a link, which could
carry a later member anywhere, a sparse file, a path given twice, or a path
under a file. web/dist and target/clax-worker are then removed and written
afresh, so nothing of the checkout's is mixed in.

This bounds where the archive writes, and nothing more. What it writes is still
that code: the Worker it deploys runs with all the Worker is given, and wrangler
reads the bundle to upload it. If Cloudflare answers 10021, a script that failed
at startup, wrangler also runs the bundle locally in workerd to profile it, in
the process tree holding the deploy's token.
"""

import os
import shutil
import stat
import sys
import tarfile

ALLOWED_DIRS = ("target/clax-worker/build", "web/dist")
ALLOWED_FILE = "target/clax-worker/built-from.json"
# What is removed before writing.
REPLACED = ("target/clax-worker", "web/dist")
# Far above a Worker's build, which Cloudflare limits to tens of megabytes.
MAX_MEMBERS = 20000
MAX_BYTES = 1 << 30


def clean(name):
    while name.startswith("./"):
        name = name[2:]
    return name.rstrip("/")


def refusal(member):
    """Why `member` may not be unpacked, or None."""
    name = clean(member.name)
    parts = name.split("/")
    if name.startswith("/") or ".." in parts or "." in parts or "" in parts:
        return "its path leaves where it may go"
    if member.issparse() or not (member.isfile() or member.isdir()):
        return "it is not a plain file or directory"
    if member.isfile() and name == ALLOWED_FILE:
        return None
    for allowed in ALLOWED_DIRS:
        if name == allowed or name.startswith(allowed + "/"):
            return None
    # The directories above them.
    if member.isdir() and name in ("target", "target/clax-worker", "web"):
        return None
    return "it is no part of a Worker's build"


def clash(members):
    """Why `members` cannot all be written, or None: writing them one by one
    would fail with the checkout already cleared."""
    seen = set()
    for member in members:
        name = clean(member.name)
        if name in seen:
            return "%r is in the archive more than once" % member.name
        seen.add(name)
    files = {clean(m.name) for m in members if m.isfile()}
    for name in seen:
        parts = name.split("/")
        for depth in range(1, len(parts)):
            if "/".join(parts[:depth]) in files:
                return "%r is under a file" % name
    return None


def the_archive(download):
    """The one file `download` may hold."""
    entries = os.listdir(download)
    path = os.path.join(download, "worker.tar")
    if entries != ["worker.tar"] or not stat.S_ISREG(os.lstat(path).st_mode):
        sys.exit("unpack-worker: the download must hold exactly one file, worker.tar")
    return path


def main(argv):
    if len(argv) != 3:
        sys.exit(__doc__)
    download, root = argv[1], argv[2]
    with tarfile.open(the_archive(download), mode="r:") as tar:
        members = []
        total = 0
        for member in tar:
            members.append(member)
            total += member.size
            if len(members) > MAX_MEMBERS:
                sys.exit("unpack-worker: too many members")
            if total > MAX_BYTES:
                sys.exit("unpack-worker: the archive is too big")
            why = refusal(member)
            if why:
                sys.exit("unpack-worker: refused %r: %s" % (member.name, why))
        why = clash(members)
        if why:
            sys.exit("unpack-worker: refused: %s" % why)
        names = {clean(m.name) for m in members}
        for needed in (ALLOWED_FILE, "target/clax-worker/build/index_bg.wasm", "web/dist/index.html"):
            if needed not in names:
                sys.exit("unpack-worker: no %s in the archive" % needed)
        for replaced in REPLACED:
            shutil.rmtree(os.path.join(root, replaced), ignore_errors=True)
        for member in members:
            path = os.path.join(root, clean(member.name))
            if member.isdir():
                os.makedirs(path, exist_ok=True)
                continue
            os.makedirs(os.path.dirname(path), exist_ok=True)
            source = tar.extractfile(member)
            with open(path, "xb") as out:
                shutil.copyfileobj(source, out)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
