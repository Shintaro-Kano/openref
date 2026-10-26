#!/usr/bin/env python3
"""OpenRef for Word の Startup テンプレート（OpenRef.dotm）を word/dotm/ から生成する。

Word も VBA エディタも使わずに .dotm を組み立てる（決定論的 = 同じ入力から同じバイト列）:
  - VBA プロジェクト（vbaProject.bin）= MS-CFB + MS-OVBA。p-code は持たず**ソースだけ**（Word が読込時にコンパイルする）
  - リボン = customUI/customUI14.xml（word/dotm/customUI14.xml）+ アイコン（word/assets/*-80.png）
  - word/vbaData.xml = 引数なしの Public Sub の一覧（Word はここに AutoExec が無いと起動時に実行しない）
VBA ソースは cp1252 でしか持てないため、日本語の文字列リテラルは ORStr(n)（ChrW の連結）に置き換え、
非 ASCII のコメントは落とす。アドインのモジュールのコンパイルエラーは「非表示モジュール」としか出ないので lint で先に止める。

  python3 scripts/build-word-dotm.py <出力.dotm>     # 生成（生成後に自己検証もする）
  python3 scripts/build-word-dotm.py --check <.dotm>  # 既存の .dotm を検証して VBA ソースを表示
  python3 scripts/build-word-dotm.py --e2e <出力.dotm> # E2E 用（word/dotm/e2e/run.sh が使う）
"""
import os
import re
import struct
import sys
import zipfile

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
SRC = os.path.join(ROOT, "word", "dotm")
ASSETS = os.path.join(ROOT, "word", "assets")
# VBA のプロジェクト名。ファイル名（OpenRef.dotm）と同じ名前にすると Word が AutoExec を見つけない
PROJECT = "OpenRefWord"
# リボンの image 属性 → word/assets のファイル
IMAGES = {"add-edit": "add-edit-80.png", "update": "update-80.png", "settings": "settings-80.png"}


# ---------------------------------------------------------------- MS-OVBA 2.4.1 圧縮 / 展開
def ovba_compress(data: bytes) -> bytes:
    out = bytearray(b"\x01")
    pos, n = 0, len(data)
    while pos < n:
        start, end = pos, min(pos + 4096, n)
        chunk = bytearray()
        cur = inserted = start
        heads = {}  # 3 バイトの頭 → 位置の列（このチャンク内）
        while cur < end:
            flag_at = len(chunk)
            chunk.append(0)
            flags = 0
            for bit in range(8):
                if cur >= end:
                    break
                while inserted < cur:
                    if inserted + 2 < end:
                        heads.setdefault(bytes(data[inserted:inserted + 3]), []).append(inserted)
                    inserted += 1
                best_len, best_pos = 0, -1
                if cur + 2 < end:
                    for cand in reversed(heads.get(bytes(data[cur:cur + 3]), ())):
                        ln = 0
                        while cur + ln < end and data[cand + ln] == data[cur + ln]:
                            ln += 1
                        if ln > best_len:
                            best_len, best_pos = ln, cand
                diff = cur - start
                bc = max((diff - 1).bit_length(), 4) if diff > 0 else 4
                if best_len >= 3 and diff > 0:
                    length = min(best_len, (0xFFFF >> bc) + 3)
                    chunk += struct.pack("<H", ((cur - best_pos - 1) << (16 - bc)) | (length - 3))
                    flags |= 1 << bit
                    cur += length
                else:
                    chunk.append(data[cur])
                    cur += 1
            chunk[flag_at] = flags
        if len(chunk) > 4096:  # 圧縮で縮まない 4096 バイト = raw チャンク
            if end - start != 4096:
                raise ValueError("圧縮できない最終チャンク")
            out += struct.pack("<H", (4096 + 2 - 3) | (0b011 << 12)) + data[start:end]
        else:
            out += struct.pack("<H", (len(chunk) + 2 - 3) | (0b011 << 12) | 0x8000) + chunk
        pos = end
    return bytes(out)


def ovba_decompress(buf: bytes) -> bytes:
    if not buf or buf[0] != 1:
        raise ValueError("CompressedContainer の署名が不正")
    out, i = bytearray(), 1
    while i < len(buf):
        hdr = struct.unpack_from("<H", buf, i)[0]
        size = (hdr & 0x0FFF) + 3
        chunk = buf[i + 2:i + size]
        i += size
        if not hdr & 0x8000:
            out += chunk[:4096]
            continue
        base, j = len(out), 0
        while j < len(chunk):
            flags = chunk[j]
            j += 1
            for bit in range(8):
                if j >= len(chunk):
                    break
                if not (flags >> bit) & 1:
                    out.append(chunk[j])
                    j += 1
                    continue
                tok = struct.unpack_from("<H", chunk, j)[0]
                j += 2
                bc = max((len(out) - base - 1).bit_length(), 4)
                off, ln = (tok >> (16 - bc)) + 1, (tok & (0xFFFF >> bc)) + 3
                for _ in range(ln):
                    out.append(out[-off])
    return bytes(out)


# ---------------------------------------------------------------- MS-OVBA 2.4.3 PROJECT の CMG / DPB / GC
def ovba_encrypt(data: bytes, project_id: str, seed: int) -> str:
    version = 2
    proj_key = sum(project_id.encode("latin-1")) & 0xFF
    venc, penc = seed ^ version, seed ^ proj_key
    out = bytearray([seed, venc, penc])
    state = [proj_key, penc, venc]  # UnencryptedByte1, EncryptedByte1, EncryptedByte2

    def emit(b):
        e = b ^ ((state[2] + state[0]) & 0xFF)
        out.append(e)
        state[2], state[1], state[0] = state[1], e, b

    for _ in range((seed & 6) // 2):
        emit(0)
    for b in struct.pack("<I", len(data)) + data:
        emit(b)
    return out.hex().upper()


# ---------------------------------------------------------------- MS-CFB（v3・512 バイトセクタ）書き出し / 読み込み
FREESECT, ENDOFCHAIN, FATSECT, NOSTREAM = 0xFFFFFFFF, 0xFFFFFFFE, 0xFFFFFFFD, 0xFFFFFFFF


def cfb_write(tree: dict) -> bytes:
    """tree = {名前: bytes（stream）| dict（storage）}"""
    entries = []

    def add(name, node):
        idx = len(entries)
        if isinstance(node, dict):
            entries.append({"name": name, "type": 1, "data": None})
            entries[idx]["kids"] = [add(k, v) for k, v in sorted(node.items(), key=lambda kv: (len(kv[0]), kv[0].upper()))]
        else:
            entries.append({"name": name, "type": 2, "data": bytes(node), "kids": []})
        return idx

    root = add("Root Entry", tree)
    entries[root]["type"] = 5
    for e in entries:
        e["left"] = e["right"] = e["child"] = NOSTREAM

    def bst(kids):  # 中央値で分ける平衡木（赤黒の色は全部黒で可）
        if not kids:
            return NOSTREAM
        m = len(kids) // 2
        entries[kids[m]]["left"], entries[kids[m]]["right"] = bst(kids[:m]), bst(kids[m + 1:])
        return kids[m]

    for e in entries:
        if e["type"] in (1, 5):
            e["child"] = bst(e["kids"])
    mini, minifat, big = bytearray(), [], []
    for e in entries:
        if e["type"] != 2:
            continue
        d = e["data"]
        if len(d) >= 4096:
            big.append(e)
            continue
        e["start"] = len(mini) // 64
        nsec = (len(d) + 63) // 64
        mini += d + b"\0" * (nsec * 64 - len(d))
        minifat += [e["start"] + k + 1 for k in range(nsec - 1)] + [ENDOFCHAIN]
    sectors, fat = [], []

    def alloc(blob):
        nsec = (len(blob) + 511) // 512
        first = len(sectors)
        for k in range(nsec):
            sectors.append(blob[k * 512:(k + 1) * 512].ljust(512, b"\0"))
            fat.append(first + k + 1 if k < nsec - 1 else ENDOFCHAIN)
        return first if nsec else ENDOFCHAIN

    for e in big:
        e["start"] = alloc(e["data"])
    entries[root]["start"], entries[root]["size"] = alloc(bytes(mini)), len(mini)
    mf = b"".join(struct.pack("<I", x) for x in minifat) + struct.pack("<I", FREESECT) * ((-len(minifat)) % 128)
    minifat_start = alloc(mf)

    def dirent(e):
        if e is None:
            return b"\0" * 64 + struct.pack("<HBBIII", 0, 0, 0, NOSTREAM, NOSTREAM, NOSTREAM) + b"\0" * 36 + struct.pack("<IQ", 0, 0)
        nm = e["name"].encode("utf-16-le") + b"\0\0"
        start, size = e.get("start", 0), e.get("size", len(e["data"] or b""))
        if e["type"] == 1:
            start, size = 0, 0
        return (nm.ljust(64, b"\0") + struct.pack("<HBB", len(nm), e["type"], 1) + struct.pack("<III", e["left"], e["right"], e["child"])
                + b"\0" * 36 + struct.pack("<IQ", start, size))

    dir_start = alloc(b"".join(dirent(e) for e in entries) + dirent(None) * ((-len(entries)) % 4))
    nfat = 1
    while (len(sectors) + nfat + 127) // 128 > nfat:
        nfat += 1
    fat_first = len(sectors)
    sectors += [None] * nfat
    fat += [FATSECT] * nfat
    fat += [FREESECT] * (nfat * 128 - len(fat))
    fb = b"".join(struct.pack("<I", x) for x in fat)
    for k in range(nfat):
        sectors[fat_first + k] = fb[k * 512:(k + 1) * 512]
    difat = [fat_first + k for k in range(nfat)] + [FREESECT] * (109 - nfat)
    hdr = (bytes.fromhex("D0CF11E0A1B11AE1") + b"\0" * 16 + struct.pack("<HHHHH", 0x003E, 3, 0xFFFE, 9, 6) + b"\0" * 6
           + struct.pack("<9I", 0, nfat, dir_start, 0, 4096, minifat_start, len(mf) // 512, ENDOFCHAIN, 0)
           + b"".join(struct.pack("<I", x) for x in difat))
    return hdr + b"".join(sectors)


def cfb_read(data: bytes) -> dict:
    """{パス: bytes} を返す（検証用）"""
    if data[:8] != bytes.fromhex("D0CF11E0A1B11AE1"):
        raise ValueError("CFB ではない")
    ssz = 1 << struct.unpack_from("<H", data, 30)[0]
    nfat, dir_start, cutoff, mf_start, _nmf, dif_start, ndif = struct.unpack_from("<II4xIII II", data, 44)
    difat = list(struct.unpack_from("<109I", data, 76))
    sec = lambda i: data[(i + 1) * ssz:(i + 2) * ssz]
    s = dif_start
    for _ in range(ndif):
        vals = struct.unpack("<%dI" % (ssz // 4), sec(s))
        difat += vals[:-1]
        s = vals[-1]
    fat = []
    for fs in [x for x in difat if x < 0xFFFFFFFA][:nfat]:
        fat += struct.unpack("<%dI" % (ssz // 4), sec(fs))

    def chain(start):
        out, seen = [], set()
        while start < 0xFFFFFFFA:
            if start in seen:
                raise ValueError("FAT が循環")
            seen.add(start)
            out.append(sec(start))
            start = fat[start]
        return b"".join(out)

    dirdata = chain(dir_start)
    ents = []
    for i in range(len(dirdata) // 128):
        e = dirdata[i * 128:(i + 1) * 128]
        nlen = struct.unpack_from("<H", e, 64)[0]
        l, r, c = struct.unpack_from("<III", e, 68)
        ents.append({"name": e[:max(nlen - 2, 0)].decode("utf-16-le"), "type": e[66], "l": l, "r": r, "c": c,
                     "start": struct.unpack_from("<I", e, 116)[0], "size": struct.unpack_from("<Q", e, 120)[0]})
    ministream = chain(ents[0]["start"]) if ents[0]["start"] < 0xFFFFFFFA else b""
    mfat = chain(mf_start) if mf_start < 0xFFFFFFFA else b""
    minifat = list(struct.unpack("<%dI" % (len(mfat) // 4), mfat))
    out = {}

    def walk(idx, prefix):
        if idx == NOSTREAM:
            return
        e = ents[idx]
        walk(e["l"], prefix)
        walk(e["r"], prefix)
        p = prefix + e["name"]
        if e["type"] == 1:
            walk(e["c"], p + "/")
        elif e["type"] == 2:
            if e["size"] < cutoff:
                parts, s2 = [], e["start"]
                while s2 < 0xFFFFFFFA:
                    parts.append(ministream[s2 * 64:(s2 + 1) * 64])
                    s2 = minifat[s2]
                out[p] = b"".join(parts)[:e["size"]]
            else:
                out[p] = chain(e["start"])[:e["size"]]

    walk(ents[0]["c"], "")
    return out


# ---------------------------------------------------------------- VBA ソースの前処理と lint
def lex_line(line):
    """1 行を [(kind, text)] に分ける。kind = code / str / comment"""
    out, i, n, buf = [], 0, len(line), ""
    while i < n:
        ch = line[i]
        if ch == '"':
            if buf:
                out.append(("code", buf))
                buf = ""
            j = i + 1
            while j < n:
                if line[j] == '"':
                    if j + 1 < n and line[j + 1] == '"':
                        j += 2
                        continue
                    break
                j += 1
            if j >= n:
                raise ValueError("文字列リテラルが閉じていない: " + line)
            out.append(("str", line[i:j + 1]))
            i = j + 1
        elif ch == "'":
            if buf:
                out.append(("code", buf))
                buf = ""
            out.append(("comment", line[i:]))
            break
        else:
            buf += ch
            i += 1
    if buf:
        out.append(("code", buf))
    return out


# 日本語環境の Mac の VBA はソースを MacJapanese として読む = 0x5C が「¥」、0x7E が「‾」になる
# （文字列の "\" が ¥ に化け、整数除算 \ はコンパイルエラー）→ この 2 文字は ChrW で作る
UNSAFE = "\\~"


class Strings:
    """日本語などを含む文字列リテラル → ORStr(n)。関数 ORStr はモジュール ORStrings に生成する
    （モジュールと同名のプロシージャは、他モジュールからの呼び出しが「モジュールを指している」コンパイルエラーになる）"""

    def __init__(self):
        self.items = []

    def ref(self, value):
        if value not in self.items:
            self.items.append(value)
        return "ORStr(%d)" % (self.items.index(value) + 1)

    @staticmethod
    def vba_expr_parts(value):
        parts, ascii_run = [], ""
        for ch in value:
            units = ch.encode("utf-16-le")
            if 32 <= ord(ch) < 127 and ch not in UNSAFE:
                ascii_run += ch
                continue
            if ascii_run:
                parts.append('"' + ascii_run.replace('"', '""') + '"')
                ascii_run = ""
            for k in range(0, len(units), 2):
                parts.append("ChrW(%d)" % struct.unpack_from("<H", units, k)[0])  # Paperpile と同じ形（ChrW$ はコンパイルエラーになった）
        if ascii_run:
            parts.append('"' + ascii_run.replace('"', '""') + '"')
        return parts or ['""']

    def module(self):
        lines = ["Option Explicit", "", "' generated by scripts/build-word-dotm.py (do not edit)"]
        per = 40  # 1 関数あたりの件数（プロシージャの大きさの上限 64KB を避ける）
        ngroups = (len(self.items) + per - 1) // per
        # 振り分けは If（Select Case の式に整数除算 \ を書くと日本語環境の Mac の VBA でコンパイルエラーになる）
        lines += ["Public Function ORStr(ByVal n As Long) As String"]
        for g in range(ngroups):
            lines.append("    %s n <= %d Then" % ("If" if g == 0 else "ElseIf", (g + 1) * per))
            lines.append("        ORStr = ORStrPart%d(n)" % (g + 1))
        lines += ["    End If", "End Function", ""]
        for g in range(ngroups):
            lines += ["Private Function ORStrPart%d(ByVal n As Long) As String" % (g + 1), "    Dim s As String", "    Select Case n"]
            for k in range(g * per, min((g + 1) * per, len(self.items))):
                parts = self.vba_expr_parts(self.items[k])
                lines.append("    Case %d" % (k + 1))
                for c in range(0, len(parts), 12):
                    lines.append("        s = %s%s" % ("s & " if c else "", " & ".join(parts[c:c + 12])))
            lines += ["    End Select", "    ORStrPart%d = s" % (g + 1), "End Function", ""]
        return "\n".join(lines) + "\n"


def vba_unquote(lit):
    return lit[1:-1].replace('""', '"')


def preprocess(name, text, strings):
    out = []
    for ln, line in enumerate(text.splitlines(), 1):
        if line.lstrip().lower().startswith("attribute "):
            continue  # VB_Name は組み立て側で付ける
        res = []
        for kind, tok in lex_line(line):
            if kind == "str" and any(ord(c) > 126 or ord(c) < 32 or c in UNSAFE for c in vba_unquote(tok)):
                if re.match(r"^\s*(Public\s+|Private\s+)?Const\b", line, re.I):
                    raise ValueError("%s:%d Const に非 ASCII の文字列は書けない（関数にする）" % (name, ln))
                res.append(strings.ref(vba_unquote(tok)))
            elif kind == "comment":
                if all(ord(c) < 127 for c in tok):
                    res.append(tok)
            else:
                res.append(tok)
        s = "".join(res).rstrip()
        if any(ord(c) > 126 for c in s):
            raise ValueError("%s:%d 文字列の外に非 ASCII がある: %s" % (name, ln, s))
        if len(s) > 1000:
            raise ValueError("%s:%d 1 行が長すぎる（VBA の上限 1023 文字）" % (name, ln))
        out.append(s)
    return "\n".join(out) + "\n"


PROC_RE = re.compile(r"^\s*(Public\s+|Private\s+|Friend\s+)?(Sub|Function)\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)", re.I | re.M)


def lint(modules, customui, project=PROJECT):
    errors, public, seen = [], {}, {}
    for name, src in modules:
        if "Option Explicit" not in src:
            errors.append("%s: Option Explicit が無い" % name)
        for m in re.finditer(r"^\s*(Public\s+|Private\s+)?Declare\s+(?!PtrSafe)", src, re.I | re.M):
            errors.append("%s: PtrSafe の無い Declare" % name)
        for ln, line in enumerate(src.splitlines(), 1):
            code = "".join(tok for kind, tok in lex_line(line) if kind == "code")
            # 整数除算 \ はソースを MacJapanese で読む VBA でコンパイルエラーになる → Int(a / b) を使う
            if any(c in code for c in UNSAFE):
                errors.append("%s:%d コードに \\ や ~ は書けない（MacJapanese で別の文字になる。整数除算は Int(a / b)）: %s" % (name, ln, line.strip()))
        for m in PROC_RE.finditer(src):
            vis, proc, args = (m.group(1) or "Public").strip().lower(), m.group(3), m.group(4).strip()
            if vis == "public":
                if proc.lower() in seen:
                    errors.append("Public %s が %s と %s で重複（あいまいな名前）" % (proc, seen[proc.lower()], name))
                seen[proc.lower()] = name
                public[proc] = args
    for cb in sorted(set(re.findall(r'\b(?:onAction|onLoad|getLabel|getEnabled|getVisible|getImage|getSupertip|getScreentip)="([^"]+)"', customui))):
        if cb not in public:
            errors.append("customUI のコールバック %s が Public Sub に無い" % cb)
        elif cb != "OR_RibbonLoad" and "IRibbonControl" not in public[cb]:
            errors.append("コールバック %s の引数が (ByVal control As IRibbonControl) でない" % cb)
    if errors:
        raise ValueError("VBA lint:\n  " + "\n  ".join(errors))
    # 引数なしの Public Sub = マクロ（vbaData.xml。AutoExec はここに無いと起動時に走らない）
    macros = []
    for name, src in modules:
        for m in PROC_RE.finditer(src):
            if (m.group(1) or "Public").strip().lower() == "public" and m.group(2).lower() == "sub" and not m.group(4).strip():
                macros.append("%s.%s.%s" % (project, name, m.group(3)))
    return macros


def load_modules(src_dir=SRC, extra_dir=None):
    mods, strings = [], Strings()
    files = [os.path.join(src_dir, f) for f in sorted(os.listdir(src_dir)) if f.endswith(".bas")]
    if extra_dir:
        files += [os.path.join(extra_dir, f) for f in sorted(os.listdir(extra_dir)) if f.endswith(".bas")]
    for path in files:
        fn = os.path.basename(path)
        name = fn[:-4]
        text = open(path, encoding="utf-8").read()
        mods.append((name, preprocess(fn, text, strings)))
    if strings.items:
        mods.append(("ORStrings", strings.module()))
    return mods


# ---------------------------------------------------------------- vbaProject.bin
REFS = [
    ("stdole", "*\\G{00020430-0000-0000-C000-000000000046}#2.0#0#C:\\Windows\\System32\\stdole2.tlb#OLE Automation"),
    ("Office", "*\\G{2DF8D04C-5BFA-101B-BDE5-00AA0044DE52}#2.0#0#C:\\Program Files\\Common Files\\Microsoft Shared\\OFFICE16\\MSO.DLL#Microsoft Office 16.0 Object Library"),
]
DOC_ATTRS = ('Attribute VB_Name = "ThisDocument"\r\nAttribute VB_Base = "0{00020906-0000-0000-C000-000000000046}"\r\n'
             "Attribute VB_GlobalNameSpace = False\r\nAttribute VB_Creatable = False\r\nAttribute VB_PredeclaredId = True\r\n"
             "Attribute VB_Exposed = True\r\nAttribute VB_TemplateDerived = False\r\nAttribute VB_Customizable = True\r\n")


def rec(rid, data=b""):
    return struct.pack("<HI", rid, len(data)) + data


def vba_project(modules, project=PROJECT, constants="") -> bytes:
    cp = "cp1252"
    mods = [("ThisDocument", "doc", DOC_ATTRS)]
    for name, src in modules:
        mods.append((name, "std", ('Attribute VB_Name = "%s"\n' % name + src).replace("\r\n", "\n").replace("\n", "\r\n")))
    u16 = lambda s: s.encode("utf-16-le")
    d = bytearray()
    d += rec(0x0001, struct.pack("<I", 3)) + rec(0x0002, struct.pack("<I", 0x0409)) + rec(0x0014, struct.pack("<I", 0x0409))
    d += rec(0x0003, struct.pack("<H", 1252)) + rec(0x0004, project.encode(cp))
    d += rec(0x0005) + rec(0x0040) + rec(0x0006) + rec(0x003D) + rec(0x0007, struct.pack("<I", 0)) + rec(0x0008, struct.pack("<I", 0))
    # 条件付きコンパイル定数（E2E ビルドだけ "OPENREF_E2E = 1"）
    d += struct.pack("<HIIH", 0x0009, 4, 0x6BEEEABD, 8) + rec(0x000C, constants.encode(cp)) + rec(0x003C, u16(constants))
    for nm, libid in REFS:
        lb = libid.encode(cp)
        d += rec(0x0016, nm.encode(cp)) + rec(0x003E, u16(nm)) + rec(0x000D, struct.pack("<I", len(lb)) + lb + struct.pack("<IH", 0, 0))
    d += rec(0x000F, struct.pack("<H", len(mods))) + rec(0x0013, struct.pack("<H", 0xFFFF))
    for name, kind, _ in mods:
        d += rec(0x0019, name.encode(cp)) + rec(0x0047, u16(name)) + rec(0x001A, name.encode(cp)) + rec(0x0032, u16(name))
        d += rec(0x001C) + rec(0x0048) + rec(0x0031, struct.pack("<I", 0)) + rec(0x001E, struct.pack("<I", 0))
        d += rec(0x002C, struct.pack("<H", 0xFFFF)) + rec(0x0022 if kind == "doc" else 0x0021) + rec(0x002B)
    d += rec(0x0010)
    vba = {"_VBA_PROJECT": b"\xcc\x61\xff\xff\x00\x00\x00", "dir": ovba_compress(bytes(d))}  # 7 バイト = p-code なし
    for name, _, text in mods:
        vba[name] = ovba_compress(text.encode(cp))
    pid = "{00000000-0000-0000-0000-000000000000}"
    lines = ['ID="%s"' % pid, "Document=ThisDocument/&H00000000"] + ["Module=%s" % n for n, k, _ in mods if k == "std"]
    lines += ['Name="%s"' % project, 'HelpContextID="0"', 'VersionCompatible32="393222000"',
              # 保護なし・パスワードなし・表示可（seed は固定 = 決定論的）
              'CMG="%s"' % ovba_encrypt(b"\0\0\0\0", pid, 0x31), 'DPB="%s"' % ovba_encrypt(b"\0", pid, 0x52),
              'GC="%s"' % ovba_encrypt(b"\xff", pid, 0x73), "", "[Host Extender Info]",
              "&H00000001={3832D640-CF90-11CF-8E43-00A0C911005A};VBE;&H00000000", "", "[Workspace]"]
    lines += ["%s=0, 0, 0, 0, C" % n for n, _, _ in mods]
    wm = b"".join(n.encode(cp) + b"\0" + u16(n) + b"\0\0" for n, _, _ in mods) + b"\0\0"
    return cfb_write({"VBA": vba, "PROJECT": ("\r\n".join(lines) + "\r\n").encode(cp), "PROJECTwm": wm})


# ---------------------------------------------------------------- OPC（.dotm）
W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'


def rels(items):
    return XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">' + "".join(
        '<Relationship Id="%s" Type="%s" Target="%s"/>' % it for it in items) + "</Relationships>"


def build(out_path, src_dir=SRC, project=PROJECT, e2e=False):
    ui_path = os.path.join(src_dir, "customUI14.xml")
    customui = open(ui_path, encoding="utf-8").read() if os.path.exists(ui_path) else ""
    modules = load_modules(src_dir, os.path.join(src_dir, "e2e") if e2e else None)
    macros = lint(modules, customui, project)
    used = set(re.findall(r'\bimage="([^"]+)"', customui))
    if used - set(IMAGES):
        raise ValueError("customUI の image が IMAGES に無い: %s" % sorted(used - set(IMAGES)))
    vba = vba_project(modules, project, "OPENREF_E2E = 1" if e2e else "")
    OFFICE = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
    parts = {
        "[Content_Types].xml": XML + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Default Extension="bin" ContentType="application/vnd.ms-office.vbaProject"/>'
        '<Default Extension="png" ContentType="image/png"/>'
        '<Override PartName="/word/document.xml" ContentType="application/vnd.ms-word.template.macroEnabledTemplate.main+xml"/>'
        '<Override PartName="/word/vbaData.xml" ContentType="application/vnd.ms-word.vbaData+xml"/>'
        '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
        '<Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/>'
        '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>'
        '<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>'
        "</Types>",
        "_rels/.rels": rels([("rId1", OFFICE + "/officeDocument", "word/document.xml"),
                             ("rId2", "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties", "docProps/core.xml"),
                             ("rId3", OFFICE + "/extended-properties", "docProps/app.xml")]
                            + ([("rId4", "http://schemas.microsoft.com/office/2007/relationships/ui/extensibility", "customUI/customUI14.xml")] if customui else [])),
        "word/document.xml": XML + '<w:document xmlns:w="%s"><w:body><w:p/><w:sectPr><w:pgSz w:w="11906" w:h="16838"/>'
        '<w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>'
        "</w:sectPr></w:body></w:document>" % W_NS,
        "word/_rels/document.xml.rels": rels([("rId1", "http://schemas.microsoft.com/office/2006/relationships/vbaProject", "vbaProject.bin"),
                                              ("rId2", OFFICE + "/styles", "styles.xml"), ("rId3", OFFICE + "/settings", "settings.xml")]),
        "word/vbaProject.bin": vba,
        "word/_rels/vbaProject.bin.rels": rels([("rId1", "http://schemas.microsoft.com/office/2006/relationships/wordVbaData", "vbaData.xml")]),
        "word/vbaData.xml": XML + '<wne:vbaSuppData xmlns:wne="http://schemas.microsoft.com/office/word/2006/wordml"><wne:mcds>'
        + "".join('<wne:mcd wne:macroName="%s" wne:name="%s" wne:bEncrypt="00" wne:cmg="56"/>' % (m.upper(), m) for m in macros)
        + "</wne:mcds></wne:vbaSuppData>",
        "word/styles.xml": XML + '<w:styles xmlns:w="%s"><w:style w:type="paragraph" w:default="1" w:styleId="Normal">'
        '<w:name w:val="Normal"/></w:style></w:styles>' % W_NS,
        "word/settings.xml": XML + '<w:settings xmlns:w="%s"><w:defaultTabStop w:val="720"/><w:compat/></w:settings>' % W_NS,
        "docProps/core.xml": XML + '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
        'xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>OpenRef for Word</dc:title><dc:creator>OpenRef</dc:creator></cp:coreProperties>',
        "docProps/app.xml": XML + '<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">'
        "<Application>OpenRef</Application></Properties>",
    }
    if customui:
        parts["customUI/customUI14.xml"] = customui
        parts["customUI/_rels/customUI14.xml.rels"] = rels([(k, OFFICE + "/image", "images/%s.png" % k) for k in sorted(used)])
        for k in sorted(used):
            parts["customUI/images/%s.png" % k] = open(os.path.join(ASSETS, IMAGES[k]), "rb").read()
    tmp = out_path + ".tmp"
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts.items():
            zi = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            zi.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(zi, data)
    check(tmp, modules)
    os.replace(tmp, out_path)
    return modules, macros


# ---------------------------------------------------------------- 検証（生成物を読み戻す）
def read_sources(path):
    with zipfile.ZipFile(path) as z:
        names = set(z.namelist())
        for need in ("word/vbaProject.bin", "word/vbaData.xml", "[Content_Types].xml"):
            if need not in names:
                raise ValueError("%s が無い" % need)
        import xml.dom.minidom

        for n in names:
            if n.endswith(".xml") or n.endswith(".rels"):
                xml.dom.minidom.parseString(z.read(n))  # well-formed か
        st = cfb_read(z.read("word/vbaProject.bin"))
    d = ovba_decompress(st["VBA/dir"])
    i, mods, cur = 0, [], {}
    while i < len(d):
        rid, sz = struct.unpack_from("<HI", d, i)
        if rid == 0x0009:
            i += 12
            continue
        val = d[i + 6:i + 6 + sz]
        i += 6 + sz
        if rid == 0x0019:
            cur = {"name": val.decode("cp1252")}
        elif rid == 0x001A:
            cur["stream"] = val.decode("cp1252")
        elif rid == 0x0031:
            cur["offset"] = struct.unpack("<I", val)[0]
        elif rid == 0x002B:
            mods.append(cur)
    return {m["name"]: ovba_decompress(st["VBA/" + m["stream"]][m["offset"]:]).decode("cp1252") for m in mods}


def check(path, modules=None):
    srcs = read_sources(path)
    if modules is not None:
        for name, src in modules:
            want = ('Attribute VB_Name = "%s"\n' % name + src).replace("\r\n", "\n").replace("\n", "\r\n")
            if srcs.get(name) != want:
                raise ValueError("読み戻した %s のソースが一致しない" % name)
    return srcs


def main(argv):
    if len(argv) == 3 and argv[1] == "--check":
        for name, src in check(argv[2]).items():
            print("==== %s (%d chars)" % (name, len(src)))
            print(src)
        return 0
    if len(argv) == 3 and argv[1] == "--e2e":
        out = argv[2]
        modules, macros = build(out, e2e=True)  # word/dotm/e2e/*.bas を同じプロジェクトに足し、OPENREF_E2E = 1 でビルド
    elif len(argv) == 2:
        out = argv[1]
        modules, macros = build(out)
    else:
        print(__doc__)
        return 2
    print("OK %s (%d bytes) modules=%s macros=%d" % (out, os.path.getsize(out), ",".join(n for n, _ in modules), len(macros)))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
