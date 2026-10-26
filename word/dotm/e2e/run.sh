#!/bin/bash
# OpenRef.dotm の E2E（実機の Microsoft Word）。
# E2E 版の OpenRef.dotm（build-word-dotm.py --e2e）を Word の Startup に置いて Word を起動し、E2E.bas の結果を表示して Word を終了する。
# Startup に元からあった OpenRef.dotm は終了時に戻す。Word が起動中なら何もしない（利用者の作業を壊さない）。
#
#   word/dotm/e2e/run.sh            # 本番の connectd（https://localhost:23120）に対して
#   word/dotm/e2e/run.sh <URL>      # 別の connectd（例: 代替ポートで起動した開発版 https://localhost:23122）
#   python3 word/dotm/e2e/mock_v2.py 23199 & word/dotm/e2e/run.sh http://127.0.0.1:23199   # 模擬（http = 表示文字列まで照合）
#   REAL_DIALOG=1 word/dotm/e2e/run.sh <URL>  # 最後に本物のダイアログ経路（OpenRef Dialog.app）を 1 回。結果の POST は別途
#   REAL_DIALOG=only FOREGROUND=1 …             # ダイアログ経路だけ・Word を前面で（前面化の確認。connectd のログの shown … active= key=）
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
BASE="${1:-}"
WD="$HOME/Library/Containers/com.microsoft.Word/Data"
GC="$HOME/Library/Group Containers/UBF8T346G9.Office"
STARTUP="$GC/User Content.localized/Startup.localized/Word"
[ -d "$STARTUP" ] || STARTUP="$GC/User Content/Startup/Word"
RESULT="$WD/tmp/openref-e2e.txt"
WORK="$(mktemp -d)"

word_pid() { ps -axo pid,comm | awk '/\/Microsoft Word$/ || / Microsoft Word$/ {print $1}' | head -1; }

if [ -n "$(word_pid)" ]; then
  echo "Word が起動中です。終了してから実行してください（E2E は Word を起動・終了します）。" >&2
  exit 2
fi
[ -d "$STARTUP" ] || { echo "Word の Startup フォルダが見つかりません: $STARTUP" >&2; exit 2; }

# E2E 版 = 本番の VBA + word/dotm/e2e/E2E.bas（同じプロジェクト・OPENREF_E2E = 1 で AutoExec が E2E.RunAll を呼ぶ）
python3 -I "$ROOT/scripts/build-word-dotm.py" --e2e "$WORK/OpenRef.dotm" >/dev/null

# 旧版（Office JS アドイン）の文書: payload は webextension パートの document settings（Office の保存形式）にだけある。
# 題名の「CT検査」= 文字参照の直後で 12 バイト目が漢字の途中に当たる並び（文字列スライスだと panic する並び = 回帰確認）
python3 -I - "$WD/Documents/openref-e2e-legacy.docx" <<'PY'
import sys, zipfile
W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R = "http://schemas.openxmlformats.org/package/2006/relationships"
we = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
      '<we:webextension xmlns:we="http://schemas.microsoft.com/office/webextensions/webextension/2010/11" id="{5E1F2A3B-4C5D-4E6F-8A9B-0C1D2E3F4A5B}">'
      '<we:reference id="1f6b8a3e-4c7d-4d2a-9b8e-5a0c2e7d9f11" version="1.2.0.0" store="developer" storeType="Registry"/>'
      '<we:alternateReferences/><we:properties>'
      '<we:property name="refract-cite-abcd1234" value="{&quot;items&quot;:[{&quot;key&quot;:&quot;PXFE4QBW&quot;,&quot;csl&quot;:{&quot;type&quot;:&quot;article-journal&quot;,&quot;title&quot;:&quot;CT検査の被ばく&quot;}}]}"/>'
      '<we:property name="refract-style" value="&quot;vancouver&quot;"/>'
      '<we:property name="refract-doi-always" value="false"/>'
      '<we:property name="refract-index" value="[&quot;refract-cite-abcd1234&quot;]"/>'
      '</we:properties><we:bindings/><we:snapshot xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"/></we:webextension>')
parts = {
    "[Content_Types].xml": '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>'
    '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
    '<Override PartName="/word/webextensions/taskpanes.xml" ContentType="application/vnd.ms-office.webextensiontaskpanes+xml"/>'
    '<Override PartName="/word/webextensions/webextension1.xml" ContentType="application/vnd.ms-office.webextension+xml"/></Types>',
    "_rels/.rels": '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="%s">'
    '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>'
    '<Relationship Id="rId2" Type="http://schemas.microsoft.com/office/2011/relationships/webextensiontaskpanes" Target="word/webextensions/taskpanes.xml"/></Relationships>' % R,
    "word/document.xml": '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="%s"><w:body>'
    '<w:p><w:r><w:t xml:space="preserve">Legacy text </w:t></w:r><w:sdt><w:sdtPr><w:alias w:val="OpenRef citation"/><w:tag w:val="refract-cite-abcd1234"/>'
    '<w:id w:val="1234567"/></w:sdtPr><w:sdtContent><w:r><w:t>[1]</w:t></w:r></w:sdtContent></w:sdt><w:r><w:t xml:space="preserve"> end.</w:t></w:r></w:p>'
    '<w:p/></w:body></w:document>' % W,
    "word/webextensions/taskpanes.xml": '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><wetp:taskpanes xmlns:wetp="http://schemas.microsoft.com/office/webextensions/taskpanes/2010/11">'
    '<wetp:taskpane dockstate="right" visibility="0" width="350" row="0"><wetp:webextensionref xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="rId1"/></wetp:taskpane></wetp:taskpanes>',
    "word/webextensions/_rels/taskpanes.xml.rels": '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="%s">'
    '<Relationship Id="rId1" Type="http://schemas.microsoft.com/office/2011/relationships/webextension" Target="webextension1.xml"/></Relationships>' % R,
    "word/webextensions/webextension1.xml": we.encode("utf-8"),
}
with zipfile.ZipFile(sys.argv[1], "w", zipfile.ZIP_DEFLATED) as z:
    for k, v in parts.items():
        z.writestr(k, v)
PY

BACKUP=""
if [ -e "$STARTUP/OpenRef.dotm" ]; then
  BACKUP="$WORK/OpenRef.dotm.orig"
  cp -p "$STARTUP/OpenRef.dotm" "$BACKUP"
fi
cleanup() {
  # KEEP_WORD=1: 失敗時に Word を残す（画面のエラーを見るため。Startup の後始末はする）
  if [ "${KEEP_WORD:-}" != 1 ] || grep -q '^DONE' "$RESULT" 2>/dev/null; then
    pid="$(word_pid)"; [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
  fi
  rm -f "$WD/Documents/openref-e2e.conf" "$WD/Documents/openref-e2e-legacy.docx"
  if [ -n "$BACKUP" ]; then cp -p "$BACKUP" "$STARTUP/OpenRef.dotm"; else rm -f "$STARTUP/OpenRef.dotm"; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

cp "$WORK/OpenRef.dotm" "$STARTUP/OpenRef.dotm"
# 2 行目 = REAL_DIALOG=1 のとき realdialog（最後に本物のダイアログ経路を 1 回通す。結果は別のシェルが connectd へ POST する）
MODE=""
[ "${REAL_DIALOG:-}" = 1 ] && MODE=realdialog
[ "${REAL_DIALOG:-}" = only ] && MODE=realdialog-only
printf '%s\n%s\n' "$BASE" "$MODE" > "$WD/Documents/openref-e2e.conf"
rm -f "$RESULT"
# 裏で起動（利用者の前面の作業を遮らない）。FOREGROUND=1 のときだけ前面（ダイアログの前面化を確かめるとき）
if [ "${FOREGROUND:-}" = 1 ]; then open -a "Microsoft Word"; else open -g -a "Microsoft Word"; fi
for _ in $(seq 1 360); do
  grep -q '^DONE' "$RESULT" 2>/dev/null && break
  sleep 0.5
done
cat "$RESULT" 2>/dev/null || echo "結果ファイルがありません（$RESULT）。Word の画面にエラーが出ていないか確認してください。"
grep -q '^DONE pass=[0-9]* fail=0$' "$RESULT" 2>/dev/null
