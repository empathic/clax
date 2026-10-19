#!/usr/bin/env python3
"""Fails on straight apostrophes and quotes, "..." and raw errors in UI copy.

    python3 scripts/check-copy.py <file.dart|file.js>...

Reads every string literal of each Dart file (single-, double- and
triple-quoted, raw included) and every string and template literal of each
JavaScript file (htm markup included), and reports, as
`path:line: <rule>: <text>`:

- straight apostrophe: a contraction or possessive written with ' (n't, 's,
  're, 'll, 've, 'd, 'm), where the copy wants ’;
- straight quotes: a word or phrase in "…" or '…' inside a literal, where
  the copy wants “…” (literals that open with { or [, JSON, are left out;
  htm attributes, `name="value"`, are not quotes);
- three dots: "...", where the copy wants …;
- raw error: a catch variable (e, err, error) interpolated into a literal,
  `$e`, `${e}`, `${error.message}`, which puts exception text on screen.

Comments, code and the expressions inside interpolations are not copy
(literals nested in an interpolation are checked as literals of their own).
Nor is a Dart literal opened right after `debugPrint(`, `print(`, a
`log(`/`_log…(` call or a core exception's constructor (`FormatException(`,
`StateError(`, `ArgumentError(`, …): those reach the log, never the screen
(`userMessage` reads every non-app error as "Something went wrong"). A JS
interpolation in attribute position (`entry=${e}`) passes a value, not text.
A line ending `// copy-check: allow` is not checked: prompt text, matchers
of stored replies, and other strings that must keep straight quotes.

Exit 0 when clean, 1 on any finding, 2 on a usage error.
"""

import re
import sys

ALLOW = "// copy-check: allow"

# A placeholder for an interpolation inside a literal's text.
HOLE = "\x00"

RULES = [
    ("straight apostrophe", re.compile(r"(?<=[A-Za-z])'(?:s|t|re|ll|ve|d|m)\b")),
    ("three dots", re.compile(r"\.\.\.")),
    (
        "straight quotes",
        re.compile(
            r"(?:(?<=^)|(?<=[\s(\[]))\"(?=[^\s\"])[^\"\n]*\""
            r"|(?:(?<=^)|(?<=[\s(\[]))'(?=[A-Za-z" + HOLE + r"])[^'\n]*'(?=[\s.,;:!?)]|$)"
        ),
    ),
]

CATCH_NAMES = ("e", "err", "error")
# A catch variable's text: bare, its message, or a type test on it
# (`${e is ApiError ? e.message : e}`); `${e.key}` and `${error ? a : b}`
# read something else.
CATCH_EXPR = re.compile(
    r"\s*(?:e|err|error)\s*(?:$|\.(?:message|toString|cause|description)\b|\bis\b)"
)

# The code before a Dart literal that makes it log or exception text.
NOT_COPY_BEFORE = re.compile(
    r"(?:\bdebugPrint|\bprint|\b_?log\w*|\b(?:Format|Unsupported)Exception"
    r"|\b(?:State|Argument|Unsupported|Unimplemented|Assertion|Range)Error)"
    r"\s*\(\s*r?$"
)


class Literal:
    """One literal's text, each character with its source line, and its holes."""

    def __init__(self, line):
        self.line = line
        self.chars = []
        self.lines = []
        self.holes = []  # (line, expression source, in attribute position)
        self.copy = True

    def add(self, ch, line):
        self.chars.append(ch)
        self.lines.append(line)

    @property
    def text(self):
        return "".join(self.chars)


class Lexer:
    def __init__(self, src):
        self.s = src
        self.i = 0
        self.line = 1
        self.literals = []

    def peek(self, k=0):
        j = self.i + k
        return self.s[j] if j < len(self.s) else ""

    def advance(self, n=1):
        for _ in range(n):
            if self.i < len(self.s):
                if self.s[self.i] == "\n":
                    self.line += 1
                self.i += 1

    def startswith(self, t):
        return self.s.startswith(t, self.i)

    def line_comment(self):
        while self.i < len(self.s) and self.s[self.i] != "\n":
            self.advance()

    def block_comment(self, nested):
        self.advance(2)
        depth = 1
        while self.i < len(self.s) and depth:
            if nested and self.startswith("/*"):
                depth += 1
                self.advance(2)
            elif self.startswith("*/"):
                depth -= 1
                self.advance(2)
            else:
                self.advance()


IDENT_CHAR = re.compile(r"[A-Za-z0-9_$]")


class DartLexer(Lexer):
    def run(self):
        self.code(stop_at_brace=False)
        return self.literals

    def code(self, stop_at_brace):
        """Lexes code up to the end, or up to the `}` closing an interpolation."""
        depth = 0
        while self.i < len(self.s):
            c = self.peek()
            if self.startswith("//"):
                self.line_comment()
            elif self.startswith("/*"):
                self.block_comment(nested=True)
            elif c in "'\"":
                raw = (
                    self.i > 0
                    and self.s[self.i - 1] == "r"
                    and (self.i < 2 or not IDENT_CHAR.match(self.s[self.i - 2]))
                )
                self.string(raw)
            elif c == "{":
                depth += 1
                self.advance()
            elif c == "}":
                if stop_at_brace and depth == 0:
                    return
                depth -= 1
                self.advance()
            else:
                self.advance()

    def string(self, raw):
        q = self.peek()
        triple = self.startswith(q * 3)
        close = q * 3 if triple else q
        line_start = self.s.rfind("\n", 0, self.i) + 1
        before = self.s[line_start : self.i]
        self.advance(len(close))
        lit = Literal(self.line)
        lit.copy = not NOT_COPY_BEFORE.search(before)
        self.literals.append(lit)
        while self.i < len(self.s):
            if self.startswith(close):
                self.advance(len(close))
                return
            c = self.peek()
            if not triple and c == "\n":
                return  # unterminated; leave it to the compiler
            if not raw and c == "\\":
                nxt = self.peek(1)
                line = self.line
                self.advance(2)
                if nxt == "$":
                    lit.add("＄", line)  # an escaped dollar is text, not a hole
                elif nxt in "'\"":
                    lit.add(nxt, line)
                else:
                    lit.add(" ", line)
                continue
            if not raw and c == "$":
                line = self.line
                if self.peek(1) == "{":
                    self.advance(2)
                    start = self.i
                    self.code(stop_at_brace=True)
                    lit.holes.append((line, self.s[start : self.i], lit.chars[-1:] == ["="]))
                    self.advance()  # the closing }
                else:
                    self.advance()
                    start = self.i
                    while self.i < len(self.s) and re.match(r"[A-Za-z0-9_]", self.peek()):
                        self.advance()
                    lit.holes.append((line, self.s[start : self.i], lit.chars[-1:] == ["="]))
                lit.add(HOLE, line)
                continue
            lit.add(c, self.line)
            self.advance()


JS_REGEX_BEFORE_WORDS = {
    "return", "typeof", "case", "in", "of", "void", "delete", "throw",
    "new", "else", "do", "yield", "await", "instanceof",
}


class JsLexer(Lexer):
    def __init__(self, src):
        super().__init__(src)
        # Whether the last significant token ends an expression (so a `/`
        # divides) or not (so a `/` opens a regular expression).
        self.after_value = False

    def run(self):
        self.code(stop_at_brace=False)
        return self.literals

    def code(self, stop_at_brace):
        depth = 0
        self.after_value = False
        while self.i < len(self.s):
            c = self.peek()
            if self.startswith("//"):
                self.line_comment()
            elif self.startswith("/*"):
                self.block_comment(nested=False)
            elif c == "/":
                if self.after_value:
                    self.advance()
                    self.after_value = False
                else:
                    self.regex()
                    self.after_value = True
            elif c in "'\"":
                self.string()
                self.after_value = True
            elif c == "`":
                self.template()
                self.after_value = True
            elif c == "{":
                depth += 1
                self.advance()
                self.after_value = False
            elif c == "}":
                if stop_at_brace and depth == 0:
                    return
                depth -= 1
                self.advance()
                self.after_value = True
            elif IDENT_CHAR.match(c):
                start = self.i
                while self.i < len(self.s) and IDENT_CHAR.match(self.peek()):
                    self.advance()
                self.after_value = self.s[start : self.i] not in JS_REGEX_BEFORE_WORDS
            elif c in ")]":
                self.advance()
                self.after_value = True
            elif c.isspace():
                self.advance()
            else:
                self.advance()
                self.after_value = False

    def regex(self):
        self.advance()
        in_class = False
        while self.i < len(self.s):
            c = self.peek()
            if c == "\n":
                return
            if c == "\\":
                self.advance(2)
                continue
            if c == "[":
                in_class = True
            elif c == "]":
                in_class = False
            elif c == "/" and not in_class:
                self.advance()
                while self.i < len(self.s) and re.match(r"[a-z]", self.peek()):
                    self.advance()
                return
            self.advance()

    def string(self):
        q = self.peek()
        self.advance()
        lit = Literal(self.line)
        self.literals.append(lit)
        while self.i < len(self.s):
            c = self.peek()
            if c == q:
                self.advance()
                return
            if c == "\n":
                return
            if c == "\\":
                nxt = self.peek(1)
                line = self.line
                self.advance(2)
                lit.add(nxt if nxt in "'\"`" else " ", line)
                continue
            lit.add(c, self.line)
            self.advance()

    def template(self):
        self.advance()
        lit = Literal(self.line)
        self.literals.append(lit)
        while self.i < len(self.s):
            c = self.peek()
            if c == "`":
                self.advance()
                return
            if c == "\\":
                nxt = self.peek(1)
                line = self.line
                self.advance(2)
                lit.add(nxt if nxt in "'\"`" else " ", line)
                continue
            if self.startswith("${"):
                line = self.line
                self.advance(2)
                start = self.i
                self.code(stop_at_brace=True)
                lit.holes.append((line, self.s[start : self.i], lit.chars[-1:] == ["="]))
                self.advance()
                lit.add(HOLE, line)
                continue
            lit.add(c, self.line)
            self.advance()


def findings(path, src):
    lexer = DartLexer(src) if path.endswith(".dart") else JsLexer(src)
    allowed = {
        n
        for n, text in enumerate(src.split("\n"), start=1)
        if text.rstrip().endswith(ALLOW)
    }
    source_lines = src.split("\n")
    out = []
    js = not path.endswith(".dart")
    for lit in lexer.run():
        if not lit.copy:
            continue
        text = lit.text
        for rule, pattern in RULES:
            if rule == "straight quotes" and text.lstrip()[:1] in ("{", "["):
                continue
            for m in pattern.finditer(text):
                out.append((lit.lines[m.start()], rule))
        for line, expr, attribute in lit.holes:
            if js and attribute:
                continue
            if expr in CATCH_NAMES or CATCH_EXPR.match(expr):
                out.append((line, "raw error"))
    seen = set()
    result = []
    for line, rule in sorted(out):
        if line in allowed or (line, rule) in seen:
            continue
        seen.add((line, rule))
        result.append(f"{path}:{line}: {rule}: {source_lines[line - 1].strip()[:120]}")
    return result


def main(argv):
    if not argv:
        print(__doc__.strip().split("\n\n")[1], file=sys.stderr)
        return 2
    bad = [p for p in argv if not p.endswith((".dart", ".js"))]
    if bad:
        print(f"check-copy: not a .dart or .js file: {', '.join(bad)}", file=sys.stderr)
        return 2
    found = []
    for path in argv:
        with open(path, encoding="utf-8") as f:
            found += findings(path, f.read())
    for f in found:
        print(f)
    if found:
        print(
            f"check-copy: {len(found)} finding(s): use ’ “ ” and …, show "
            "userMessage(e) rather than the error, or end the line "
            f"`{ALLOW}` where the text must stay as it is",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
