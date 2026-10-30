#!/bin/bash
# install.sh / uninstall.sh の「Claude Desktop(チャット)の MCP 登録」を、偽の HOME で回す試験。
# 本物の ~/Library/Application Support/Claude には触れない(Desktop の起動判定も関数の差し替え・偽の ps で行う)。
# 前提: macOS(osascript)。使い方: bash scripts/test-install-desktop-mcp.sh(scripts/test-all.sh からも呼ばれる)
set -u
cd "$(dirname "$0")/.."
REPO="$PWD"
T="$(mktemp -d)"
cleanup() { chmod -R u+rwx "$T" 2>/dev/null || true; rm -rf "$T"; }
trap cleanup EXIT
RESULTS="$T/results"; : > "$RESULTS"   # 各ケースはサブシェルで回すので、結果はファイルに積んで最後に数える
ok()  { echo ok   >> "$RESULTS"; echo "  ok   $1"; }
bad() { echo FAIL >> "$RESULTS"; echo "  FAIL $1"; }
# $1 = 名前、$2 = 条件(eval)
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }
# $1 = 名前、$2 = 期待する部分文字列、$3 = 実際の出力
has() { case "$3" in *"$2"*) ok "$1" ;; *) bad "$1 (出力: $3)" ;; esac; }

# 関数だけを読み込む(末尾の main "$@" を落とす)。macOS の bash 3.2 は source <(…) で関数が入らないことがある → ファイル経由。
# 最終行が main "$@" でなければ止める(main が走ると /Applications や launchctl の本物に触る)
for f in site/install.sh site/uninstall.sh; do
  [ "$(tail -n 1 "$REPO/$f")" = 'main "$@"' ] || { echo "ABORT: $f の最終行が main \"\$@\" ではない(main を落とせない)" >&2; exit 99; }
done
sed '$d' "$REPO/site/install.sh"   > "$T/inst.sh"
sed '$d' "$REPO/site/uninstall.sh" > "$T/uninst.sh"
load_install()   { source "$T/inst.sh"; }
load_uninstall() { source "$T/uninst.sh"; }
not_running() { claude_desktop_running() { return 1; }; }
running()     { claude_desktop_running() { return 0; }; }
BIN='/Users/testuser/Library/Application Support/OpenRef/bin/openref-mcp'
cfg_of() { echo "$1/Library/Application Support/Claude/claude_desktop_config.json"; }
# $1 = ファイル、$2 = JS 式(d = 設定オブジェクト)
jget() {
  OPENREF_T="$(cat "$1")" osascript -l JavaScript -e 'ObjC.import("stdlib"); function run(a){ const d=JSON.parse(String($.getenv("OPENREF_T"))); return String(new Function("d","return ("+a[0]+")")(d)); }' -- "$2"
}
# 新しい偽 HOME(Claude フォルダ付き)を作って HOME と C(設定ファイルのパス)を設定する。$(…) の中で呼ばない(HOME が親に残らない)
new_home() { HOME="$T/$1"; export HOME; mkdir -p "$HOME/Library/Application Support/Claude"; C="$(cfg_of "$HOME")"; }
# 本物の HOME に触る事故を止める(new_home を $(…) の中で呼ぶと HOME が親に残らない → ここで必ず止まる)
guard() { case "$HOME" in "$T"/*) ;; *) echo "ABORT: HOME is not under the test dir: $HOME" >&2; kill -TERM $$; exit 99 ;; esac; }
# 各ケースは ( … ) のサブシェルで、source した set -e が効いた状態で回す。関数が非 0 を返すとサブシェルが黙って終わるので、終了コードを次の行で見る
# (「( … ) || bad」の形にすると、|| の左側では set -e が無効になって中断が起きなくなる)
ended() { [ "$1" = 0 ] || bad "case aborted: a function returned non-zero under set -e (exit $1)"; }
reg()   { guard; register_claude_desktop "$@"; }
unreg() { guard; unregister_claude_desktop "$@"; }

echo "== 共通ブロックは install.sh と uninstall.sh で同一"
blk() { sed -n '/^# ---- Claude Desktop(チャット)の MCP 登録(ここから/,/^# ---- Claude Desktop(チャット)の MCP 登録: ここまで ----$/p' "$1"; }
blk "$REPO/site/install.sh" > "$T/blk1"; blk "$REPO/site/uninstall.sh" > "$T/blk2"
check "block present (>40 lines)" '[ "$(wc -l < "$T/blk1")" -gt 40 ]'
check "block identical" 'cmp -s "$T/blk1" "$T/blk2"'

echo "== install: Claude Desktop 未導入(フォルダ無し)→ スキップ・何も作らない"
( HOME="$T/h1"; export HOME; mkdir -p "$HOME"; load_install; not_running
  out="$(reg "$BIN")"; has "skip message" "スキップ" "$out"
  check "no file created" '[ ! -e "$(cfg_of "$HOME")" ]' )
ended $?

echo "== install: 設定ファイル無し → 新規作成(0600・tmp を残さない)"
( new_home h2; load_install; not_running
  out="$(reg "$BIN")"; has "registered message" "登録しました" "$out"
  check "command" '[ "$(jget "$C" "d.mcpServers.openref.command")" = "$BIN" ]'
  check "args []" '[ "$(jget "$C" "JSON.stringify(d.mcpServers.openref.args)")" = "[]" ]'
  check "mode 0600" '[ "$(stat -f %Lp "$C")" = "600" ]'
  check "no tmp left" '[ -z "$(ls "$C".tmp-openref* 2>/dev/null)" ]'
  check "no backup when there was no file" '[ ! -e "$C.bak-openref" ]' )
ended $?

echo "== install: 他の登録と設定がある → 保持して追加・キー順を保つ・退避・再実行は変更なし"
( new_home h3; load_install; not_running
  printf '{\n  "preferences": {"theme": "dark"},\n  "mcpServers": {\n    "notion": {"command": "npx", "args": ["-y", "@notionhq/notion-mcp-server"], "env": {"X": "日本語"}}\n  },\n  "coworkUserFilesPath": "/Users/t/x"\n}\n' > "$C"
  before="$(cat "$C")"
  out="$(reg "$BIN")"; has "registered" "登録しました" "$out"
  check "notion kept" '[ "$(jget "$C" "d.mcpServers.notion.command")" = "npx" ]'
  check "utf8 kept" '[ "$(jget "$C" "d.mcpServers.notion.env.X")" = "日本語" ]'
  check "preferences kept" '[ "$(jget "$C" "d.preferences.theme")" = "dark" ]'
  check "other key kept" '[ "$(jget "$C" "d.coworkUserFilesPath")" = "/Users/t/x" ]'
  check "key order kept (preferences first)" '[ "$(jget "$C" "Object.keys(d)[0]")" = "preferences" ]'
  check "openref added" '[ "$(jget "$C" "d.mcpServers.openref.command")" = "$BIN" ]'
  check "backup = previous content" '[ "$(cat "$C.bak-openref")" = "$before" ]'
  m1="$(stat -f %m "$C")"; sleep 1.1
  out2="$(reg "$BIN")"; has "second run: already" "登録済み" "$out2"
  check "second run: file untouched" '[ "$m1" = "$(stat -f %m "$C")" ]' )
ended $?

echo "== install: 既存 openref の env / args を保って command だけ向け直す・mcpServers が配列なら作り直す"
( new_home h4; load_install; not_running
  echo '{"mcpServers":{"openref":{"command":"/old/openref-mcp","args":["--x"],"env":{"OPENREF_EMAIL":"a@b"}}}}' > "$C"
  reg "$BIN" >/dev/null
  check "path updated" '[ "$(jget "$C" "d.mcpServers.openref.command")" = "$BIN" ]'
  check "env kept" '[ "$(jget "$C" "d.mcpServers.openref.env.OPENREF_EMAIL")" = "a@b" ]'
  check "args kept" '[ "$(jget "$C" "JSON.stringify(d.mcpServers.openref.args)")" = "[\"--x\"]" ]'
  out="$(reg "$BIN")"; has "re-run with env: unchanged" "登録済み" "$out"
  echo '{"mcpServers":{"openref":"oops"}}' > "$C"
  reg "$BIN" >/dev/null
  check "non-object openref replaced" '[ "$(jget "$C" "d.mcpServers.openref.command")" = "$BIN" ]'
  echo '{"mcpServers":[1,2]}' > "$C"
  reg "$BIN" >/dev/null
  check "array replaced" '[ "$(jget "$C" "d.mcpServers.openref.command")" = "$BIN" ]' )
ended $?

echo "== install: 空 / 空白だけのファイルは {} として作る"
( new_home h5; load_install; not_running
  : > "$C"; out="$(reg "$BIN")"; has "empty: registered" "登録しました" "$out"
  check "empty: command" '[ "$(jget "$C" "d.mcpServers.openref.command")" = "$BIN" ]'
  printf ' \n\n' > "$C"; out="$(reg "$BIN")"; has "blank: registered" "登録しました" "$out" )
ended $?

echo "== install: 触ってはいけない中身 → 触らない(壊れた JSON・UTF-8 でない・NUL・配列・シンボリックリンク)"
( new_home h6; load_install; not_running
  for case in 'bad:{bad json' 'array:[1,2]' 'string:"x"'; do
    name="${case%%:*}"; printf '%s' "${case#*:}" > "$C"; rm -f "$C.bak-openref"
    out="$(reg "$BIN")"; has "$name: warn" "手を付けませんでした" "$out"
    check "$name: untouched" '[ "$(cat "$C")" = "${case#*:}" ]'
    check "$name: no backup" '[ ! -e "$C.bak-openref" ]'
  done
  printf '\xff\xfe{"mcpServers":{"other":{"command":"x"}}}' > "$C"; rm -f "$C.bak-openref"
  out="$(reg "$BIN")"; has "invalid utf-8: warn" "手を付けませんでした" "$out"
  check "invalid utf-8: untouched" 'cmp -s "$C" <(printf "\xff\xfe{\"mcpServers\":{\"other\":{\"command\":\"x\"}}}")'
  printf '\0\0\0\0' > "$C"
  out="$(reg "$BIN")"; has "nul bytes: warn" "手を付けませんでした" "$out"
  check "nul bytes: untouched" '[ "$(wc -c < "$C" | tr -d " ")" = "4" ]'
  check "nul bytes: no tmp" '[ -z "$(ls "$C".tmp-openref* 2>/dev/null)" ]'
  printf '{"mcpServers":{"a":{"command":"x\0y"}}}' > "$C"
  out="$(reg "$BIN")"; has "nul inside: warn" "NUL" "$out"
  check "nul inside: untouched" '[ "$(wc -c < "$C" | tr -d " ")" = "38" ]'
  echo '{"mcpServers":{}}' > "$T/h6/real.json"; rm -f "$C"; ln -s "$T/h6/real.json" "$C"
  out="$(reg "$BIN")"; has "symlink: warn" "シンボリックリンク" "$out"
  check "symlink: still a symlink" '[ -L "$C" ]'
  check "symlink: target untouched" '[ "$(cat "$T/h6/real.json")" = "{\"mcpServers\":{}}" ]' )
ended $?

echo "== install: 読めない・書けない → 案内して止まらない"
( new_home h7; load_install; not_running
  echo '{"mcpServers":{}}' > "$C"; chmod 000 "$C"
  out="$(reg "$BIN")"; has "unreadable: warn" "読めません" "$out"
  chmod 644 "$C"; check "unreadable: untouched" '[ "$(cat "$C")" = "{\"mcpServers\":{}}" ]'
  chmod 555 "$(dirname "$C")"
  out="$(reg "$BIN")"; has "unwritable dir: cannot back up → warn, not written" "退避できません" "$out"
  chmod 755 "$(dirname "$C")"
  check "unwritable dir: untouched" '[ "$(cat "$C")" = "{\"mcpServers\":{}}" ]'
  check "unwritable dir: no tmp" '[ -z "$(ls "$C".tmp-openref* 2>/dev/null)" ]'
  rm -f "$C" "$C.bak-openref"; chmod 555 "$(dirname "$C")"
  out="$(reg "$BIN")"; chmod 755 "$(dirname "$C")"
  has "unwritable dir, no config: warn" "書き込めません" "$out"
  check "unwritable dir, no config: nothing created" '[ -z "$(ls "$(dirname "$C")")" ]'
  echo '{"mcpServers":{}}' > "$C"; echo '{"x":1}' > "$T/h7/elsewhere.json"
  ln -s "$T/h7/elsewhere.json" "$C.bak-openref"
  out="$(reg "$BIN")"; has "symlinked backup: registered" "登録しました" "$out"
  check "symlinked backup: link replaced by a file" '[ ! -L "$C.bak-openref" ] && [ "$(cat "$C.bak-openref")" = "{\"mcpServers\":{}}" ]'
  check "symlinked backup: target untouched" '[ "$(cat "$T/h7/elsewhere.json")" = "{\"x\":1}" ]'
  check "config is a regular file" '[ ! -L "$C" ] && [ "$(stat -f %Lp "$C")" = "600" ]'
  chmod 000 "$C"; chmod 555 "$(dirname "$C")"
  out="$(reg "$BIN")"; chmod 755 "$(dirname "$C")"; chmod 644 "$C"
  has "cannot back up: warn" "読めません" "$out" )
ended $?

echo "== install: 起動中 → 触らない / set -e の下で関数が 0 を返す"
( new_home h8; load_install; running
  printf '{"mcpServers":{}}' > "$C"
  out="$(reg "$BIN")"; has "running: warn" "起動中" "$out"
  check "running: untouched" '[ "$(cat "$C")" = "{\"mcpServers\":{}}" ]'
  set -e; reg "$BIN" >/dev/null; ok "survived running under set -e"
  not_running; printf '{bad' > "$C"; reg "$BIN" >/dev/null; ok "survived invalid json under set -e"
  chmod 000 "$C"; reg "$BIN" >/dev/null; ok "survived unreadable under set -e"; chmod 644 "$C" )
ended $?

echo "== 起動判定: 偽の ps(本物のプロセス一覧は見ない)"
( load_install
  ps() { printf '/sbin/launchd\n/Applications/Claude.app/Contents/MacOS/Claude\n/usr/sbin/cfprefsd\n'; }
  check "running: full path in the middle" 'claude_desktop_running'
  ps() { printf '/sbin/launchd\n/Applications/Claude.app/Contents/MacOS/Claude'; }
  check "running: full path at the end" 'claude_desktop_running'
  ps() { printf '/sbin/launchd\n/Applications/Claude.app/Contents/MacOS/Claude Helper\n/Applications/Claude.app/Contents/Frameworks/Claude Helper (GPU).app/Contents/MacOS/Claude Helper (GPU)\n'; }
  check "not running: helpers only" '! claude_desktop_running'
  ps() { printf '/sbin/launchd\n/Applications/Microsoft Word.app/Contents/MacOS/Microsoft Word\n'; }
  check "not running: others" '! claude_desktop_running'
  ps() { return 1; }
  check "ps failed → treated as running (safe side)" 'claude_desktop_running'
  ps() { :; }
  check "ps empty → treated as running (safe side)" 'claude_desktop_running' )
ended $?

echo "== uninstall: openref だけ外す / 無ければ何もしない / 起動中は案内 / 壊れた JSON / ファイル無し"
( new_home h9; load_uninstall; not_running
  printf '{"mcpServers":{"notion":{"command":"npx","args":[]},"openref":{"command":"%s","args":[]}},"preferences":{"a":1}}' "$BIN" > "$C"
  out="$(unreg)"; has "removed message" "外しました" "$out"
  check "openref gone" '[ "$(jget "$C" "d.mcpServers.openref")" = "undefined" ]'
  check "notion kept" '[ "$(jget "$C" "d.mcpServers.notion.command")" = "npx" ]'
  check "preferences kept" '[ "$(jget "$C" "d.preferences.a")" = "1" ]'
  check "backup made" '[ -s "$C.bak-openref" ]'
  m1="$(stat -f %m "$C")"; sleep 1.1
  out="$(unreg)"; check "absent: silent" '[ -z "$out" ]'
  check "absent: untouched" '[ "$m1" = "$(stat -f %m "$C")" ]'
  printf '{"mcpServers":{"openref":{"command":"x","args":[]}}}' > "$C"; running
  out="$(unreg)"; has "running: notice" "起動中" "$out"; has "running: tells to re-run" "もう一度" "$out"
  check "running: untouched" '[ "$(jget "$C" "d.mcpServers.openref.command")" = "x" ]'
  not_running; printf '{openref bad' > "$C"
  out="$(unreg)"; has "invalid: notice" "手を付けませんでした" "$out"
  check "invalid: untouched" '[ "$(cat "$C")" = "{openref bad" ]'
  rm -f "$C"; out="$(unreg)"; check "no file: nothing" '[ -z "$out" ] && [ ! -e "$C" ]'
  set -e; printf '{openref bad' > "$C"; unreg >/dev/null; ok "survived invalid json under set -e" )
ended $?

echo "== 構文"
check "bash -n install.sh" 'bash -n "$REPO/site/install.sh"'
check "bash -n uninstall.sh" 'bash -n "$REPO/site/uninstall.sh"'

echo
pass="$(grep -c '^ok$' "$RESULTS")"; fail="$(grep -c '^FAIL$' "$RESULTS")"
EXPECT=87
echo "desktop-mcp: $pass ok, $fail failed (expected $EXPECT checks)"
[ "$fail" = 0 ] && [ "$((pass+fail))" = "$EXPECT" ]
