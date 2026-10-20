#!/usr/bin/env python3
"""The Clax app's Dart import graph, as Bazel targets.

    dart_graph.py APP_DIR            prints graph.bzl (bazel/flutter/dart_graph.bzl
                                     runs this at load time)
    dart_graph.py APP_DIR --report   prints the folder graph, its cycles, the
                                     libraries and the test suites, for a person

Reads every `import`, `export` and `part` of the app's Dart files (lib/,
test/, tool/, integration_test/) and derives:

- LIBS: a library per Dart library under lib/, a file, or a set of files
  that import each other in a cycle (Bazel targets cannot form one). Each
  names the libraries and pub packages its files import.
- SUITES: the test files (test/**/*_test.dart) in groups that share one
  compile (bazel/flutter/tests.bzl). Each names the test-side files its
  tests reach (test support, tool/, integration_test/), the libraries those
  files import, the pub packages they import, and whether they show the
  licences (NOTICES_READER). A test is built against
  what it imports, directly or through those files, and nothing else, so
  `bazel query rdeps` from a changed file reaches only the tests whose
  group imports it.

The groups trade selection for disk: a compile carries Flutter and the
reached app (60 to 90 MB), so one per test file would cost gigabytes. The
*_real_test.dart files form a group of their own (they run the real daemon
and core); the others are merged, closest first, into SUITE_COUNT groups:
each step merges the two groups whose union adds the fewest (file, test)
pairs in which the test is selected by the file without reaching it.
"""

import heapq
import os
import re
import sys
from collections import Counter, OrderedDict

# The groups of the non-real tests, each a compile of its own.
SUITE_COUNT = 15

# The app's file that shows the licences page, whose LicenseRegistry reads
# the bundle's NOTICES.Z: a suite that reaches it, or has a test-side file
# that names NOTICES, gets //apps/clax_app:notices among its assets.
NOTICES_READER = "lib/ui/licences.dart"

ROOTS = ["lib", "test", "tool", "integration_test"]
DIRECTIVE = re.compile(r"^\s*(import|export|part)\s+'([^']+)'", re.M)
PACKAGE = "clax_app"


def fail(msg):
    sys.stderr.write("dart_graph: %s\n" % msg)
    sys.exit(1)


def read_graph(app):
    """{file: ([local files], [pub packages], [part files])}, paths relative to app."""
    files = []
    for root in ROOTS:
        for dirpath, dirnames, names in os.walk(os.path.join(app, root)):
            dirnames.sort()
            for n in sorted(names):
                if n.endswith(".dart"):
                    files.append(os.path.relpath(os.path.join(dirpath, n), app))
    graph = {}
    for f in files:
        with open(os.path.join(app, f), encoding="utf-8") as fh:
            src = fh.read()
        local, pub, parts = [], [], []
        for kind, uri in DIRECTIVE.findall(src):
            if uri.startswith("dart:"):
                continue
            if uri.startswith("package:"):
                pkg, _, rest = uri[len("package:"):].partition("/")
                if pkg != PACKAGE:
                    pub.append(pkg)
                    continue
                target = os.path.normpath("lib/" + rest)
            else:
                target = os.path.normpath(os.path.join(os.path.dirname(f), uri))
            (parts if kind == "part" else local).append(target)
        graph[f] = (local, pub, parts)
    for f, (local, _, parts) in graph.items():
        for t in local + parts:
            if t not in graph:
                fail("%s imports %s, which is not a Dart file of the app" % (f, t))
    return graph


def sccs(nodes, edges):
    """Tarjan's strongly connected components, iteratively, in a fixed order."""
    index, low, on, stack, out = {}, {}, set(), [], []
    counter = [0]
    for root in nodes:
        if root in index:
            continue
        work = [(root, iter(edges[root]))]
        index[root] = low[root] = counter[0]
        counter[0] += 1
        stack.append(root)
        on.add(root)
        while work:
            v, it = work[-1]
            pushed = False
            for w in it:
                if w not in index:
                    index[w] = low[w] = counter[0]
                    counter[0] += 1
                    stack.append(w)
                    on.add(w)
                    work.append((w, iter(edges[w])))
                    pushed = True
                    break
                if w in on:
                    low[v] = min(low[v], index[w])
            if pushed:
                continue
            work.pop()
            if work:
                low[work[-1][0]] = min(low[work[-1][0]], low[v])
            if low[v] == index[v]:
                comp = []
                while True:
                    w = stack.pop()
                    on.discard(w)
                    comp.append(w)
                    if w == v:
                        break
                out.append(sorted(comp))
    return out


def lib_name(path):
    return "lib_" + path[len("lib/"):-len(".dart")].replace("/", "_")


def build(app):
    graph = read_graph(app)
    sources = {}
    for f in graph:
        with open(os.path.join(app, f), encoding="utf-8") as fh:
            sources[f] = fh.read()
    libs = sorted(f for f in graph if f.startswith("lib/"))

    # A part is one library with its owner: an edge each way.
    edges = {f: [] for f in graph}
    for f, (local, _, parts) in graph.items():
        edges[f] += local + parts
        for p in parts:
            edges[p].append(f)
    lib_edges = {f: [t for t in edges[f] if t.startswith("lib/")] for f in libs}
    for f in libs:
        for t in edges[f]:
            if not t.startswith("lib/"):
                fail("%s, under lib/, imports %s, which is not" % (f, t))

    owner = {}
    targets = OrderedDict()
    for comp in sorted(sccs(libs, lib_edges)):
        name = lib_name(comp[0])
        if name in targets:
            fail("two libraries are named %s: %s and %s" % (name, targets[name]["srcs"], comp))
        targets[name] = {"srcs": comp}
        for f in comp:
            owner[f] = name
    for name, t in targets.items():
        deps, pub = set(), set()
        for f in t["srcs"]:
            deps.update(owner[d] for d in lib_edges[f])
            pub.update(graph[f][1])
        deps.discard(name)
        t["deps"] = sorted(deps)
        t["pub"] = sorted(pub)

    def closure(f):
        seen, todo = set(), [f]
        while todo:
            x = todo.pop()
            if x not in seen:
                seen.add(x)
                todo.extend(edges[x])
        return frozenset(seen)

    tests = sorted(f for f in graph if f.startswith("test/") and f.endswith("_test.dart"))
    reach = {t: closure(t) for t in tests}
    real = [t for t in tests if t.endswith("_real_test.dart")]
    groups = [g for g in cluster([t for t in tests if t not in real], reach) if g]

    suites = OrderedDict()
    if real:
        suites["tests_real"] = real
    named = Counter()
    for g in sorted(groups):
        dirs = Counter(t.split("/")[1] if t.count("/") > 1 else "root" for t in g)
        top = sorted(dirs.items(), key=lambda kv: (-kv[1], kv[0]))[0][0]
        named[top] += 1
        suites[(top, named[top])] = g
    out = OrderedDict()
    for key, members in suites.items():
        if isinstance(key, tuple):
            top, n = key
            key = "tests_%s" % top if named[top] == 1 else "tests_%s_%d" % (top, n)
        files = set().union(*(reach[t] for t in members))
        side = sorted(f for f in files if not f.startswith("lib/"))
        deps, pub = set(), set()
        for f in side:
            deps.update(owner[d] for d in edges[f] if d.startswith("lib/"))
            pub.update(graph[f][1])
        notices = NOTICES_READER in files or any("NOTICES" in sources[f] for f in side)
        out[key] = {
            "notices": notices,
            "tests": sorted(members),
            "srcs": [f for f in side if f not in members],
            "deps": sorted(deps),
            "pub": sorted(pub),
            "real": key == "tests_real",
        }
    return graph, targets, out, reach


def cluster(tests, reach):
    """Merges tests into SUITE_COUNT groups; see the module's docstring."""
    files = sorted(set().union(*(reach[t] for t in tests))) if tests else []
    bit = {f: 1 << i for i, f in enumerate(files)}

    def mask(fs):
        m = 0
        for f in fs:
            m |= bit[f]
        return m

    def ones(m):
        return bin(m).count("1")

    same = OrderedDict()
    for t in tests:
        same.setdefault(reach[t], []).append(t)
    groups = {i: (ts, mask(c)) for i, (c, ts) in enumerate(same.items())}

    def cost(x, y):
        (ta, ua), (tb, ub) = groups[x], groups[y]
        return ones(ua | ub) * (len(ta) + len(tb)) - ones(ua) * len(ta) - ones(ub) * len(tb)

    keys = sorted(groups)
    heap = [(cost(a, b), a, b) for i, a in enumerate(keys) for b in keys[i + 1:]]
    heapq.heapify(heap)
    nxt = len(groups)
    while len(groups) > SUITE_COUNT:
        _, x, y = heapq.heappop(heap)
        if x not in groups or y not in groups:
            continue
        groups[nxt] = (groups[x][0] + groups[y][0], groups[x][1] | groups[y][1])
        del groups[x], groups[y]
        for z in sorted(groups):
            if z != nxt:
                heapq.heappush(heap, (cost(z, nxt), z, nxt))
        nxt += 1
    return [sorted(ts) for ts, _ in groups.values()]


def starlark(value, indent=0):
    pad = "    " * (indent + 1)
    end = "    " * indent
    if isinstance(value, bool):
        return "True" if value else "False"
    if isinstance(value, str):
        return '"%s"' % value
    if isinstance(value, list):
        if not value:
            return "[]"
        return "[\n" + "".join("%s%s,\n" % (pad, starlark(v, indent + 1)) for v in value) + end + "]"
    if isinstance(value, dict):
        return "{\n" + "".join(
            "%s%s: %s,\n" % (pad, starlark(k), starlark(v, indent + 1)) for k, v in value.items()
        ) + end + "}"
    raise TypeError(value)


def emit(targets, suites):
    return (
        "# Generated by bazel/flutter/dart_graph.py from the app's imports; never edited.\n\n"
        "# {name: {srcs, deps (other LIBS), pub (pub packages)}}: lib/ by its imports.\n"
        "LIBS = %s\n\n"
        "# {name: {tests, srcs (test-side files they reach), deps (LIBS), pub, real}}.\n"
        "SUITES = %s\n" % (starlark(targets), starlark(suites))
    )


def report(graph, targets, suites, reach):
    def folder(f):
        return os.path.dirname(f)

    fedges = {}
    for f in graph:
        if f.startswith("lib/"):
            fedges.setdefault(folder(f), set())
            for t in graph[f][0]:
                if t.startswith("lib/") and folder(t) != folder(f):
                    fedges[folder(f)].add(folder(t))
    nodes = sorted(fedges)
    print("Folders of lib/ and the folders they import:")
    for n in nodes:
        print("  %s -> %s" % (n, ", ".join(sorted(fedges[n])) or "-"))
    print("Folder cycles:")
    for comp in sccs(nodes, {n: sorted(fedges[n]) for n in nodes}):
        if len(comp) > 1:
            print("  " + ", ".join(comp))
    print("File cycles (each one library):")
    for name, t in targets.items():
        if len(t["srcs"]) > 1:
            print("  %s: %s" % (name, ", ".join(t["srcs"])))
    print("%d libraries; %d suites:" % (len(targets), len(suites)))
    for name, s in suites.items():
        files = set().union(*(reach[t] for t in s["tests"]))
        print("  %s: %d tests, %d lib files" % (name, len(s["tests"]), len([f for f in files if f.startswith("lib/")])))


def main(argv):
    if len(argv) < 2:
        fail("usage: dart_graph.py APP_DIR [--report]")
    graph, targets, suites, reach = build(argv[1])
    if "--report" in argv[2:]:
        report(graph, targets, suites, reach)
    else:
        sys.stdout.write(emit(targets, suites))


if __name__ == "__main__":
    main(sys.argv)
