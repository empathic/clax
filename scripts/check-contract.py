#!/usr/bin/env python3
"""Fails a change to the API contract that breaks a client of the last
release without raising the contract's major version (spec
2026-10-27-clax-sdk-design §4.1).

  check-contract.py                     against the newest v* tag
  check-contract.py --base <ref>        against <ref>'s contract
  check-contract.py --old A --new B     between two contract files

The contract is web/sdk/openapi.json, which crates/clax-server generates
from the routes. With --base, a ref that holds none passes: there is
nothing to break. Exits 1 naming each break when the new contract's
info.version has the old one's major (or a lower one), 2 on bad usage.

A break is what a client written against the old contract could see fail:
- an operation gone (by method and path), or its operationId, success
  status or body media type changed; `x-clax-retry` turned off; a channel
  it accepted gone;
- in what a client reads (answers): a field gone, or no longer always
  present; a type or enum value it may now hold that it could not;
- in what a client sends (requests, parameters): a field now required, or
  gone where unknown fields are refused; a type or enum value it could
  send that is now refused.
New operations, optional request fields, answer fields, error codes and
descriptions break nothing.
"""

import json
import os
import re
import subprocess
import sys

CONTRACT = "web/sdk/openapi.json"
REF = "#/components/schemas/"


def major(version):
    m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", version or "")
    if not m:
        sys.exit(f"check-contract: {version!r} is no X.Y.Z version")
    return int(m.group(1))


class Compare:
    def __init__(self, old, new):
        self.old, self.new = old, new
        self.breaks = []
        self.seen = set()

    def resolve(self, doc, s):
        while isinstance(s, dict) and "$ref" in s:
            ref = s["$ref"]
            if not ref.startswith(REF):
                sys.exit(f"check-contract: a $ref outside the components: {ref}")
            extra = {k: v for k, v in s.items() if k != "$ref"}
            s = dict(doc["components"]["schemas"][ref[len(REF):]], **extra)
        return s if isinstance(s, dict) else {}

    @staticmethod
    def types(s):
        """The JSON types a schema admits, `None` for any."""
        alts = s.get("anyOf") or s.get("oneOf")
        if alts:
            out = set()
            for a in alts:
                t = Compare.types(a)
                if t is None:
                    return None
                out |= t
            return out
        if "const" in s:
            return {type(s["const"]).__name__}
        t = s.get("type")
        if t is None:
            return {"object"} if "properties" in s else None
        ts = set(t if isinstance(t, list) else [t])
        # An integer is a number.
        if "number" in ts:
            ts.add("integer")
        return ts

    def compare(self, o, n, reading, at):
        """`o` and `n` describe one value; `reading` when a client reads it."""
        key = (json.dumps(o, sort_keys=True), json.dumps(n, sort_keys=True), reading)
        if key in self.seen:
            return
        self.seen.add(key)
        o, n = self.resolve(self.old, o), self.resolve(self.new, n)
        ot, nt = self.types(o), self.types(n)
        # Reading, the new may hold no type the old could not; sending, the
        # new must take every type the old took.
        wider, narrower = (nt, ot) if reading else (ot, nt)
        if narrower is not None and (wider is None or not wider <= narrower):
            more = "any type" if wider is None else ", ".join(sorted(wider - narrower))
            self.brk(at, f"{'may now hold' if reading else 'no longer takes'} {more}")
        oe, ne = o.get("enum"), n.get("enum")
        if "const" in o:
            oe = [o["const"]]
        if "const" in n:
            ne = [n["const"]]
        if reading and oe is not None and ne is not None:
            added = [v for v in ne if v not in oe]
            if added:
                self.brk(at, f"may now hold {', '.join(map(json.dumps, added))}")
        if reading and oe is not None and ne is None:
            self.brk(at, f"may now hold values besides {', '.join(map(json.dumps, oe))}")
        if not reading and oe is not None:
            gone = [v for v in oe if ne is not None and v not in ne]
            if gone:
                self.brk(at, f"no longer takes {', '.join(map(json.dumps, gone))}")
        if not reading and oe is None and ne is not None:
            self.brk(at, "now takes only " + ", ".join(map(json.dumps, ne)))
        if "items" in o and "items" in n:
            self.compare(o["items"], n["items"], reading, at + "[]")
        op, np_ = o.get("properties", {}), n.get("properties", {})
        oreq, nreq = set(o.get("required", [])), set(n.get("required", []))
        for name, s in op.items():
            if name not in np_:
                if reading:
                    self.brk(f"{at}.{name}", "is gone")
                elif n.get("additionalProperties") is False:
                    self.brk(f"{at}.{name}", "is gone, and unknown fields are refused")
                continue
            if reading and name in oreq and name not in nreq:
                self.brk(f"{at}.{name}", "is no longer always present")
            self.compare(s, np_[name], reading, f"{at}.{name}")
        if not reading:
            for name in sorted(nreq - oreq):
                self.brk(f"{at}.{name}", "is now required")
        oa, na = o.get("additionalProperties"), n.get("additionalProperties")
        if isinstance(oa, dict) and isinstance(na, dict):
            self.compare(oa, na, reading, at + "{}")
        for k in ("contentSchema",):
            if k in o and k in n:
                self.compare(o[k], n[k], reading, at)

    def brk(self, at, what):
        self.breaks.append(f"{at} {what}")

    def content(self, oc, nc, reading, at):
        oc, nc = oc or {}, nc or {}
        for media, o in oc.items():
            if media not in nc:
                self.brk(at, f"no longer is {media}")
                continue
            self.compare(o.get("schema", {}), nc[media].get("schema", {}), reading, at)

    def operations(self):
        for path, item in self.old.get("paths", {}).items():
            for method, o in item.items():
                at = f"{method.upper()} {path}"
                n = self.new.get("paths", {}).get(path, {}).get(method)
                if n is None:
                    self.brk(at, "is gone")
                    continue
                if o.get("operationId") != n.get("operationId"):
                    self.brk(at, f"is now operationId {n.get('operationId')!r}")
                if o.get("x-clax-retry") and not n.get("x-clax-retry"):
                    self.brk(at, "may no longer be retried")
                for ch in o.get("x-clax-channel", []):
                    if ch not in n.get("x-clax-channel", []):
                        self.brk(at, f"no longer accepts the {ch} channel")
                ok = lambda x: sorted(s for s in x.get("responses", {}) if int(s) < 400)
                if ok(o) != ok(n):
                    self.brk(at, f"answers {', '.join(ok(n))}, not {', '.join(ok(o))}")
                else:
                    for st in ok(o):
                        self.content(o["responses"][st].get("content"), n["responses"][st].get("content"), True, f"{at} {st}")
                if "requestBody" in o or "requestBody" in n:
                    if "requestBody" not in n:
                        self.brk(at, "takes no body any more")
                    elif "requestBody" not in o:
                        if n["requestBody"].get("required"):
                            self.brk(at, "now requires a body")
                    else:
                        self.content(o["requestBody"].get("content"), n["requestBody"].get("content"), False, f"{at} body")
                op = {(p["in"], p["name"]): p for p in o.get("parameters", [])}
                for p in n.get("parameters", []):
                    k = (p["in"], p["name"])
                    if k not in op:
                        if p.get("required"):
                            self.brk(f"{at} {p['in']} {p['name']}", "is new and required")
                        continue
                    self.compare(op[k].get("schema", {}), p.get("schema", {}), False, f"{at} {p['in']} {p['name']}")
        return self.breaks


def check(old, new, base):
    breaks = Compare(old, new).operations()
    ov, nv = old["info"]["version"], new["info"]["version"]
    if major(nv) < major(ov):
        sys.exit(f"check-contract: the contract's version {nv} is below {base}'s {ov}")
    if breaks and major(nv) == major(ov):
        print(f"check-contract: the contract breaks clients of {base} ({ov}) without a major version (now {nv}):", file=sys.stderr)
        for b in breaks:
            print(f"  {b}", file=sys.stderr)
        print("Raise clax_core::contract::VERSION's major (and MAJOR), or keep the old shape.", file=sys.stderr)
        sys.exit(1)
    what = f"{len(breaks)} break(s), major raised" if breaks else "no break"
    print(f"check-contract: {CONTRACT} {nv} against {base} ({ov}): {what}")


def newest_tag():
    tags = subprocess.run(["git", "tag", "-l", "v*"], capture_output=True, text=True, check=True).stdout.split()
    tags = [t for t in tags if re.fullmatch(r"v\d+\.\d+\.\d+", t)]
    if not tags:
        return None
    return max(tags, key=lambda t: tuple(int(x) for x in t[1:].split(".")))


def main(argv):
    args = dict(zip(argv[::2], argv[1::2]))
    if len(argv) % 2 or not set(args) <= {"--base", "--old", "--new"} or ("--old" in args) != ("--new" in args):
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        sys.exit(2)
    if "--old" in args:
        with open(args["--old"]) as f:
            old = json.load(f)
        with open(args["--new"]) as f:
            new = json.load(f)
        return check(old, new, args["--old"])
    os.chdir(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    base = args.get("--base") or newest_tag()
    if base is None:
        print("check-contract: no v* tag to compare with")
        return
    shown = subprocess.run(["git", "show", f"{base}:{CONTRACT}"], capture_output=True, text=True)
    if shown.returncode != 0:
        print(f"check-contract: {base} has no {CONTRACT}; nothing to break")
        return
    with open(CONTRACT) as f:
        new = json.load(f)
    check(json.loads(shown.stdout), new, base)


if __name__ == "__main__":
    main(sys.argv[1:])
