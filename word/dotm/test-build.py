#!/usr/bin/env python3
"""scripts/build-word-dotm.py のテスト（Word なしで回る分。実機の Word での確認は word/dotm/e2e/run.sh）。
  python3 word/dotm/test-build.py
Word for Mac の落とし穴を lint が止めること・生成物が読み戻せて決定論的であることを確かめる。"""
import importlib.util
import os
import random
import re
import sys
import tempfile
import zipfile

sys.dont_write_bytecode = True  # scripts/__pycache__ を作らない
ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
spec = importlib.util.spec_from_file_location("bwd", os.path.join(ROOT, "scripts", "build-word-dotm.py"))
B = importlib.util.module_from_spec(spec)
spec.loader.exec_module(B)

failed = 0


def check(name, ok, detail=""):
    global failed
    print(("ok   " if ok else "FAIL ") + name + ("" if ok else "  :: " + str(detail)))
    if not ok:
        failed += 1


def lint_error(mods, ui=""):
    try:
        B.lint(mods, ui)
        return ""
    except ValueError as e:
        return str(e)


def eval_vba_str(expr):
    """ORStrings が生成した 's = ... & ChrW(n) & "..."' の右辺を Python で評価する"""
    out = ""
    for tok in re.findall(r'ChrW\((\d+)\)|"((?:[^"]|"")*)"', expr):
        out += chr(int(tok[0])) if tok[0] else tok[1].replace('""', '"')
    return out


# ---- OVBA 圧縮の往復（乱数・繰り返し・チャンク境界） ----
rnd = random.Random(7)
samples = [b"", b"a", b"abc" * 3000, bytes(rnd.randrange(256) for _ in range(8192))]
src = open(os.path.join(ROOT, "word", "dotm", "ORDoc.bas"), "rb").read()
samples += [src[:n] for n in (4095, 4096, 4097, 8192, len(src))]
check("ovba compress roundtrip", all(B.ovba_decompress(B.ovba_compress(s)) == s for s in samples if s))

# ---- 本番ビルド: 読み戻し・決定論・中身 ----
with tempfile.TemporaryDirectory() as tmp:
    a, b = os.path.join(tmp, "a.dotm"), os.path.join(tmp, "b.dotm")
    modules, _ = B.build(a)
    B.build(b)
    check("deterministic bytes", open(a, "rb").read() == open(b, "rb").read())
    srcs = B.check(a, modules)
    joined = "".join(srcs.values())
    check("compiled sources are ASCII without \\ and ~", all(32 <= ord(c) < 127 or c in "\r\n\t" for c in joined) and "\\" not in joined and "~" not in joined)
    check("project name differs from file name", B.PROJECT != "OpenRef", B.PROJECT)
    with zipfile.ZipFile(a) as z:
        vbadata = z.read("word/vbaData.xml").decode()
        ui = z.read("customUI/customUI14.xml").decode()
        dirs = B.ovba_decompress(B.cfb_read(z.read("word/vbaProject.bin"))["VBA/dir"])
    check("vbaData lists AutoExec", ('wne:macroName="%s"' % (B.PROJECT + ".ORRibbon.AutoExec").upper()) in vbadata, vbadata)
    check("ribbon tab after References", 'insertAfterMso="TabReferences"' in ui)
    check("production build has no E2E constant", b"OPENREF_E2E" not in dirs)
    # ORStr の生成式が元の文字列に戻る
    strings = B.Strings()
    for fn in sorted(os.listdir(os.path.join(ROOT, "word", "dotm"))):
        if fn.endswith(".bas"):
            B.preprocess(fn, open(os.path.join(ROOT, "word", "dotm", fn), encoding="utf-8").read(), strings)
    gen = strings.module()
    cases, cur = {}, None
    for line in gen.splitlines():
        m = re.match(r"\s*Case (\d+)$", line)
        if m:
            cur = int(m.group(1))
            cases[cur] = ""
            continue
        m = re.match(r"\s*s = (s & )?(.*)$", line)
        if m and cur is not None:
            cases[cur] += eval_vba_str(m.group(2))
    check("ORStr expressions decode to the source strings", [cases.get(i + 1) for i in range(len(strings.items))] == strings.items,
          [(i + 1, strings.items[i], cases.get(i + 1)) for i in range(len(strings.items)) if cases.get(i + 1) != strings.items[i]][:3])
    # E2E ビルドは定数と E2E モジュールを持つ
    e = os.path.join(tmp, "e.dotm")
    emods, _ = B.build(e, e2e=True)
    with zipfile.ZipFile(e) as z:
        edir = B.ovba_decompress(B.cfb_read(z.read("word/vbaProject.bin"))["VBA/dir"])
    check("e2e build has the constant and the E2E module", b"OPENREF_E2E = 1" in edir and "E2E" in [n for n, _ in emods])

# ---- lint（Word for Mac の落とし穴） ----
ok_mod = ("M", "Option Explicit\nPublic Sub Foo()\nEnd Sub\n")
check("lint passes a clean module", lint_error([ok_mod]) == "")
check("lint rejects backslash in code", "\\" in lint_error([("M", "Option Explicit\nPublic Function F() As Long\n    F = 7 \\ 2\nEnd Function\n")]))
check("lint rejects missing Option Explicit", "Option Explicit" in lint_error([("M", "Public Sub Foo()\nEnd Sub\n")]))
check("lint rejects Declare without PtrSafe", "PtrSafe" in lint_error([("M", 'Option Explicit\nPrivate Declare Function f Lib "x" () As Long\n')]))
check("lint rejects duplicate Public names", "重複" in lint_error([ok_mod, ("N", "Option Explicit\nPublic Sub Foo()\nEnd Sub\n")]))
check("lint rejects a missing ribbon callback", "Bar" in lint_error([ok_mod], '<button onAction="Bar"/>'))
check("lint rejects a callback with the wrong signature", "IRibbonControl" in lint_error([ok_mod], '<button onAction="Foo"/>'))
try:
    B.preprocess("X.bas", 'Option Explicit\nPrivate Const A As String = "日本"\n', B.Strings())
    check("preprocess rejects non-ASCII Const", False)
except ValueError:
    check("preprocess rejects non-ASCII Const", True)
st = B.Strings()
out = B.preprocess("X.bas", 'Option Explicit\n\' 日本語のコメント\nx = "a\\b" & "c~d" & "日本" \' 末尾のコメント\n', st)
check("preprocess moves \\ ~ and non-ASCII literals to ORStr and drops non-ASCII comments",
      out == 'Option Explicit\n\nx = ORStr(1) & ORStr(2) & ORStr(3)\n' and st.items == ["a\\b", "c~d", "日本"], repr(out))

print("FAILED: %d" % failed if failed else "all passed")
sys.exit(1 if failed else 0)
