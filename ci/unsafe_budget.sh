#!/usr/bin/env bash
# Unsafe budget enforcement (CLAUDE.md hard rules):
#
#   1. `unsafe` appears only in .rs files under kernel/src/hal/
#   2. at most 4 files in hal contain `unsafe`
#   3. at most 200 total lines inside unsafe blocks/fns
#   4. every `unsafe` is directly preceded by a `// SAFETY:` comment
#      (clippy's undocumented_unsafe_blocks=deny in kernel/Cargo.toml is the
#      authoritative check; this is the toolchain-free backstop)
#   5. every non-hal .rs file carries `#![forbid(unsafe_code)]`
#      (the crate root may use `#![deny(unsafe_code)]` instead, because
#      `forbid` there could not be re-allowed for the hal child module)
#
# Static analysis only — runs with just bash + python3, no Rust toolchain, so
# CI can run it before (and independently of) the build. Trivially green
# while no kernel sources exist yet (M0).
#
# Never weaken this script to make a red budget pass. Shrink the unsafe.

set -euo pipefail
cd "$(dirname "$0")/.."

if ! compgen -G "kernel/src/*.rs" > /dev/null && ! find kernel/src -name '*.rs' -print -quit 2>/dev/null | grep -q .; then
    echo "unsafe_budget: no kernel sources yet — budget trivially OK (0/200 lines, 0/4 files)"
    exit 0
fi

python3 - <<'EOF'
import pathlib
import re
import sys

MAX_UNSAFE_FILES = 4
MAX_UNSAFE_LINES = 200
HAL = pathlib.Path("kernel/src/hal")

failures = []


def strip_comments_and_strings(src: str) -> str:
    """Blank out comments, string literals (incl. raw/byte strings), and
    char literals, preserving newlines and brace structure so line numbers
    and block extents survive. Raw strings matter: r#"..."# has no escapes,
    and mishandling one would desync the scanner and blank out real code —
    an attacker's path to hiding `unsafe`."""
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        two = src[i:i + 2]
        raw = re.match(r'b?r(#*)"', src[i:])
        if raw and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            # Raw (byte) string: contents end at `"` + same number of `#`;
            # backslashes are literal, never escapes.
            close = src.find('"' + raw.group(1), i + raw.end())
            end = n if close == -1 else close + 1 + len(raw.group(1))
            out.append("".join(ch if ch == "\n" else " " for ch in src[i:end]))
            i = end
        elif two == "//":
            j = src.find("\n", i)
            j = n if j == -1 else j
            out.append(" " * (j - i))
            i = j
        elif two == "/*":
            depth, j = 1, i + 2
            while j < n and depth:
                if src[j:j + 2] == "/*":
                    depth += 1
                    j += 2
                elif src[j:j + 2] == "*/":
                    depth -= 1
                    j += 2
                else:
                    j += 1
            out.append("".join(ch if ch == "\n" else " " for ch in src[i:j]))
            i = j
        elif c == '"':
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                elif src[j] == '"':
                    j += 1
                    break
                else:
                    j += 1
            out.append('"' + "".join(ch if ch == "\n" else " " for ch in src[i + 1:j - 1]) + '"')
            i = j
        elif c == "'" and re.match(r"'(\\.|[^\\'])'", src[i:i + 4]):
            m = re.match(r"'(\\.|[^\\'])'", src[i:])
            out.append(" " * len(m.group(0)))
            i += len(m.group(0))
        else:
            out.append(c)
            i += 1
    return "".join(out)


def unsafe_spans(clean: str):
    """Yield (start_line, end_line) for each unsafe block/fn (1-based).

    Rust 2024 unsafe attributes — `#[unsafe(no_mangle)]` — carry the token
    but have no body; they count as their own single line."""
    for m in re.finditer(r"\bunsafe\b", clean):
        rest = clean[m.end():].lstrip()
        line = clean.count("\n", 0, m.start()) + 1
        if rest.startswith("("):
            yield (line, line)
            continue
        open_brace = clean.find("{", m.end())
        if open_brace == -1:
            continue
        depth, j = 1, open_brace + 1
        while j < len(clean) and depth:
            if clean[j] == "{":
                depth += 1
            elif clean[j] == "}":
                depth -= 1
            j += 1
        yield (line, clean.count("\n", 0, j) + 1)


def has_safety_comment(raw_lines, unsafe_line):
    """A `// SAFETY:` comment block must sit directly above the unsafe line."""
    i = unsafe_line - 2  # index of the line above (0-based)
    while i >= 0:
        stripped = raw_lines[i].strip()
        if stripped.startswith("// SAFETY:"):
            return True
        if stripped.startswith("//") or stripped.startswith("#["):
            i -= 1  # continuation of a comment block / attribute; keep looking
            continue
        return False
    return False


unsafe_files = []
total_unsafe_lines = 0

for path in sorted(pathlib.Path("kernel/src").rglob("*.rs")):
    raw = path.read_text()
    clean = strip_comments_and_strings(raw)
    in_hal = HAL in path.parents

    spans = []
    for start, end in unsafe_spans(clean):
        spans.append((start, end))
        if not has_safety_comment(raw.splitlines(), start):
            failures.append(f"{path}:{start}: unsafe without a `// SAFETY:` comment directly above")

    if re.search(r"\bunsafe\b", clean):
        if not in_hal:
            failures.append(f"{path}: `unsafe` outside kernel/src/hal/")
        else:
            unsafe_files.append(path)

    if in_hal and spans:
        # merge overlapping spans (nested unsafe counts once)
        spans.sort()
        merged = [list(spans[0])]
        for s, e in spans[1:]:
            if s <= merged[-1][1]:
                merged[-1][1] = max(merged[-1][1], e)
            else:
                merged.append([s, e])
        total_unsafe_lines += sum(e - s + 1 for s, e in merged)

    if not in_hal:
        # Match against `clean`, not `raw`: the attribute inside a comment or
        # string is not seen by rustc and must not satisfy this check.
        is_crate_root = path.name in ("main.rs", "lib.rs") and path.parent.name == "src"
        lints = "forbid|deny" if is_crate_root else "forbid"
        if not re.search(rf"^\s*#!\[({lints})\(unsafe_code\)\]", clean, re.M):
            want = "#![forbid(unsafe_code)]" + (" or #![deny(unsafe_code)]" if is_crate_root else "")
            failures.append(f"{path}: missing {want} (as a real attribute, not a comment)")

if len(unsafe_files) > MAX_UNSAFE_FILES:
    failures.append(
        f"{len(unsafe_files)} hal files contain unsafe (max {MAX_UNSAFE_FILES}): "
        + ", ".join(str(p) for p in unsafe_files))
if total_unsafe_lines > MAX_UNSAFE_LINES:
    failures.append(f"{total_unsafe_lines} lines inside unsafe blocks (max {MAX_UNSAFE_LINES})")

if failures:
    print("unsafe_budget: FAIL")
    for f in failures:
        print(f"  {f}")
    sys.exit(1)

print(f"unsafe_budget: OK — {total_unsafe_lines}/{MAX_UNSAFE_LINES} unsafe lines "
      f"in {len(unsafe_files)}/{MAX_UNSAFE_FILES} hal files")
EOF
