#!/usr/bin/env python3
"""Comment budget + slop scan. py/ts/tsx/js/jsx/rs + md. `--strict` fails warnings.
Auto-discovers source under cwd. No layout assumptions. Args override discovery."""

from __future__ import annotations

import ast
import io
import re
import sys
import tokenize
from dataclasses import dataclass
from pathlib import Path

SKIP = {
    ".venv",
    "__pycache__",
    ".git",
    ".pytest_cache",
    ".ruff_cache",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    "coverage",
}
CODE = {".py", ".ts", ".tsx", ".js", ".jsx", ".rs"}

FILE_WARN, FILE_FAIL = 0.15, 0.20
TREE_WARN, TREE_FAIL = 0.13, 0.15
BLOCK_WARN, BLOCK_FAIL = 2, 4
# article density: articles per word. caveman ~0.05, prose ~0.14
COMMENT_ART, MD_ART = 0.18, 0.12
MIN_WORDS = 8

# first arg of an LLM tool handler; its docstring is machine-read schema, not prose
TOOL_PARAM = "FunctionCallParams"

# grows when sweep finds new tic
TELLS = (
    r"which is why",
    r"that is the whole reason",
    r"the whole point",
    r"not a preference",
    r"worth its line",
    r"load-bearing",
    r"which is what",
    r"that is how",
    r"— so ",
    r"needless to say",
    r"in other words",
)
TELL = re.compile("|".join(TELLS), re.I)
ARTICLE = re.compile(r"\b(the|a|an)\b", re.I)
WORD = re.compile(r"[a-zA-Z']+")


@dataclass(frozen=True, slots=True)
class Lang:
    line: tuple[str, ...]  # line-comment starts
    block: tuple[tuple[str, str], ...]  # (open, close) block comments
    strings: tuple[str, ...]  # string delims to skip


CLIKE = Lang(line=("//",), block=(("/*", "*/"),), strings=('"', "'", "`"))
RUST = Lang(line=("//",), block=(("/*", "*/"),), strings=('"',))
LANGS = {".ts": CLIKE, ".tsx": CLIKE, ".js": CLIKE, ".jsx": CLIKE, ".rs": RUST}


@dataclass(frozen=True, slots=True)
class Block:
    line: int
    lines: int


@dataclass(frozen=True, slots=True)
class Prose:
    line: int
    text: str


@dataclass(frozen=True, slots=True)
class Report:
    path: Path
    chars: int
    prose: int
    blocks: list[Block]
    texts: list[Prose]

    @property
    def ratio(self) -> float:
        return self.prose / self.chars if self.chars else 0.0


def _slop(text: str) -> tuple[re.Match | None, float, int]:
    words = WORD.findall(text)
    art = len(ARTICLE.findall(text)) / len(words) if words else 0.0
    return TELL.search(text), art, len(words)


# --- python: ast docstrings + tokenize comments, tool-schema docstrings skipped ---


def _named(annotation: ast.expr | None) -> str:
    if isinstance(annotation, ast.Constant):
        return str(annotation.value).rpartition(".")[2]
    return getattr(annotation, "id", getattr(annotation, "attr", ""))


def _schema(node: ast.AST) -> bool:
    # tool handler docstring is parsed into what the model reads: interface, not prose
    args = getattr(getattr(node, "args", None), "args", [])[:2]
    return any(_named(a.annotation) == TOOL_PARAM for a in args)


def _py_docstrings(tree: ast.AST) -> list[ast.Constant]:
    out = []
    for node in ast.walk(tree):
        body = getattr(node, "body", None)
        if not isinstance(body, list) or not body or _schema(node):
            continue
        first = body[0]
        if isinstance(first, ast.Expr) and isinstance(first.value, ast.Constant):
            if isinstance(first.value.value, str):
                out.append(first.value)
    return out


def _py_comments(src: str) -> tuple[list[Prose], list[Block]]:
    lines = src.splitlines()
    runs: list[Block] = []
    texts: list[Prose] = []
    prev = None
    for tok in tokenize.generate_tokens(io.StringIO(src).readline):
        if tok.type != tokenize.COMMENT:
            continue
        line = tok.start[0]
        texts.append(Prose(line, tok.string))
        # comment sharing line with code = margin note, not block
        if lines[line - 1][: tok.start[1]].strip():
            prev = None
            continue
        if prev is not None and line == prev + 1:
            runs[-1] = Block(runs[-1].line, runs[-1].lines + 1)
        else:
            runs.append(Block(line, 1))
        prev = line
    return texts, runs


def _extract_py(src: str) -> tuple[list[Prose], list[Block]]:
    tree = ast.parse(src)
    texts, blocks = _py_comments(src)
    for node in _py_docstrings(tree):
        seg = ast.get_source_segment(src, node) or ""
        blocks.append(Block(node.lineno, node.end_lineno - node.lineno + 1))
        texts.append(Prose(node.lineno, seg))
    return texts, sorted(blocks, key=lambda b: b.line)


# --- generic: single pass, skip strings, collect line + block comments ---


def _match(src: str, i: int, toks) -> str | None:
    for tok in sorted(toks, key=len, reverse=True):  # longest wins
        if src.startswith(tok, i):
            return tok
    return None


def _skip_string(src: str, i: int, delim: str) -> int:
    n = len(src)
    while i < n:
        if src[i] == "\\":
            i += 2
            continue
        if src.startswith(delim, i):
            return i + len(delim)
        if delim in ("'", '"') and src[i] == "\n":
            return i  # unterminated single-line string
        i += 1
    return n


def _extract_generic(src: str, lang: Lang) -> tuple[list[Prose], list[Block]]:
    line_no, i, n = 1, 0, len(src)
    texts: list[Prose] = []
    runs: list[Block] = []
    prev_run = None  # last line-comment line, for merging consecutive
    while i < n:
        if src[i] == "\n":
            line_no += 1
            i += 1
            continue
        if block := _match(src, i, [o for o, _ in lang.block]):
            close = next(c for o, c in lang.block if o == block)
            end = src.find(close, i + len(block))
            end = n if end < 0 else end + len(close)
            seg = src[i:end]
            texts.append(Prose(line_no, seg))
            runs.append(Block(line_no, seg.count("\n") + 1))
            line_no += seg.count("\n")
            i, prev_run = end, None
        elif _match(src, i, lang.line):
            end = src.find("\n", i)
            end = n if end < 0 else end
            texts.append(Prose(line_no, src[i:end]))
            if src[src.rfind("\n", 0, i) + 1 : i].strip():  # code before comment = margin
                prev_run = None
            elif prev_run is not None and line_no == prev_run + 1:
                runs[-1] = Block(runs[-1].line, runs[-1].lines + 1)
                prev_run = line_no
            else:
                runs.append(Block(line_no, 1))
                prev_run = line_no
            i = end
        elif sd := _match(src, i, lang.strings):
            i, prev_run = _skip_string(src, i + len(sd), sd), None
        else:
            i += 1
    return texts, runs


def scan(path: Path) -> Report | None:
    src = path.read_text()
    if path.suffix == ".py":
        try:
            texts, blocks = _extract_py(src)
        except SyntaxError:
            return None
    else:
        texts, blocks = _extract_generic(src, LANGS[path.suffix])
    return Report(path, len(src), sum(len(t.text) for t in texts), blocks, texts)


def _collect(bases: list[Path], suffixes: set[str], scaffold: bool = False) -> list[Path]:
    out: list[Path] = []
    for base in bases:
        if base.is_file():
            if base.suffix in suffixes:
                out.append(base)
            continue
        for p in base.rglob("*"):
            # _-prefixed doc = scaffold placeholder, consumer deletes it. code keeps dunders
            if SKIP & set(p.parts) or (scaffold and p.name.startswith("_")):
                continue
            if p.suffix in suffixes:
                out.append(p)
    return sorted(set(out))


def sources(argv: list[str]) -> list[Path]:
    return _collect([Path(a) for a in argv] if argv else [Path()], CODE)


def docs(argv: list[str]) -> list[Path]:
    # any dir named docs, anywhere; excludes .claude command docs (slop-tell literals)
    bases = (
        [Path(a) for a in argv]
        if argv
        else [d for d in Path().rglob("docs") if d.is_dir() and not SKIP & set(d.parts)]
    )
    return _collect(bases, {".md"}, scaffold=True)


def scan_md(path: Path, warns: list[str], fails: list[str]) -> None:
    text = path.read_text()
    for i, line in enumerate(text.splitlines(), 1):
        if m := TELL.search(line):
            fails.append(f"{path}:{i}: slop tell: {m.group()!r}")
    _, art, words = _slop(text)
    if words >= MIN_WORDS and art > MD_ART:
        warns.append(f"{path}: {art:.0%} articles, over {MD_ART:.0%} — prose voice")


def main() -> None:
    argv = [a for a in sys.argv[1:] if a != "--strict"]
    strict = "--strict" in sys.argv
    warns: list[str] = []
    fails: list[str] = []

    reports = []
    for p in sources(argv):
        if (r := scan(p)) is None:
            warns.append(f"{p}: unparseable, skipped")
        else:
            reports.append(r)

    for r in reports:
        pct = f"{r.ratio:.1%}"
        if r.ratio > FILE_FAIL:
            fails.append(f"{r.path}: {pct} prose, over {FILE_FAIL:.0%}")
        elif r.ratio > FILE_WARN:
            warns.append(f"{r.path}: {pct} prose, over {FILE_WARN:.0%}")
        for b in r.blocks:
            where = f"{r.path}:{b.line}"
            if b.lines > BLOCK_FAIL:
                fails.append(f"{where}: {b.lines}-line block, over {BLOCK_FAIL}")
            elif b.lines > BLOCK_WARN:
                warns.append(f"{where}: {b.lines}-line block, over {BLOCK_WARN}")
        for t in r.texts:
            tell, art, words = _slop(t.text)
            where = f"{r.path}:{t.line}"
            if tell:
                fails.append(f"{where}: slop tell: {tell.group()!r}")
            elif words >= MIN_WORDS and art > COMMENT_ART:
                warns.append(f"{where}: {art:.0%} articles, over {COMMENT_ART:.0%} — prose voice")

    for p in docs(argv):
        scan_md(p, warns, fails)

    chars = sum(r.chars for r in reports)
    prose = sum(r.prose for r in reports)
    tree = prose / chars if chars else 0.0
    if tree > TREE_FAIL:
        fails.append(f"tree: {tree:.1%} prose, over {TREE_FAIL:.0%}")
    elif tree > TREE_WARN:
        warns.append(f"tree: {tree:.1%} prose, over {TREE_WARN:.0%}")

    for line in fails:
        print(f"FAIL {line}")
    for line in warns:
        print(f"warn {line}")
    print(f"{len(reports)} files, tree {tree:.1%} prose")
    if fails or warns:
        print("rewrite honestly per /deslop. no synonym-swaps.")

    if fails or (strict and warns):
        sys.exit(1)


if __name__ == "__main__":
    main()
