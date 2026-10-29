"""The composer's Python under Bazel (//web/reel:reel_python): the pinned
CPython with numpy and Pillow, taking the command lines compose.sh gives a
`python3`: `-I -c CODE ARG...`, `-I - ARG...` (the program on stdin) and
`-I SCRIPT ARG...`. `-I` is accepted and ignored: the interpreter already
reads only its own packages. A program runs in this process's `__main__`,
as under `python3`, and what it leaves open (compose.sh's cut writer does
not close its files) is closed, and so written out, when it ends.
"""

import gc
import runpy
import sys


def main(args):
    collect = gc.collect  # the program's run clears this module's names
    if args[:1] == ["-I"]:
        args = args[1:]
    if not args:
        sys.exit("python.py: no program (-c CODE, - or a script)")
    if args[0] in ("-c", "-"):
        code = args[1] if args[0] == "-c" else sys.stdin.read()
        rest = args[2:] if args[0] == "-c" else args[1:]
        sys.argv = [args[0]] + rest
        scope = sys.modules["__main__"].__dict__
        exec(compile(code, "<string>" if args[0] == "-c" else "<stdin>", "exec"), scope)
        scope.clear()
    else:
        sys.argv = args
        runpy.run_path(args[0], run_name="__main__")
    collect()


if __name__ == "__main__":
    main(sys.argv[1:])
