#!/bin/bash
# OpenRef アンインストーラ — https://openref.unitedradiology.jp
# アプリ・常駐サービス・拡張・Word 連携(Startup の OpenRef.dotm・検索画面・旧 Office アドイン manifest)を削除します
# (改名前の Refract の名前で入っているものも)。
# ライブラリデータ(~/Library/Application Support/OpenRef。旧版のままなら …/Refract)は消しません。
# Claude Desktop の設定(claude_desktop_config.json)からは mcpServers.openref だけを外します(Desktop 起動中は外せないので案内だけ)。
set -euo pipefail

# ---- Claude Desktop(チャット)の MCP 登録(ここから「ここまで」の行までは install.sh と uninstall.sh で同一。試験 = scripts/test-install-desktop-mcp.sh)----
# Claude Desktop は起動時に claude_desktop_config.json をメモリへ読み、終了時にメモリの内容でファイルを丸ごと書き戻す。
# 起動中に書き足した項目は終了時に消えるので、起動中は書かない。JSON の編集は osascript(JavaScript)= 全 Mac に標準で、追加の導入が要らない。
claude_desktop_running() {
  # ps の出力を変数に取ってから見る(pipefail の下で grep -q が先に終わると ps が SIGPIPE で落ちて判定を誤る)。
  # ps が失敗した・何も返さないときは「起動中かもしれない」= 書かない側に倒す。見るのは自分のプロセスだけ(別ユーザーの Desktop は無関係)
  PROCS="$(ps -U "$(id -u)" -xo comm= 2>/dev/null)" || return 0
  [ -n "$PROCS" ] || return 0
  case "$PROCS" in
    *"/Claude.app/Contents/MacOS/Claude"|*"/Claude.app/Contents/MacOS/Claude"$'\n'*) return 0 ;;
  esac
  return 1
}
# $1 = add | remove、$2 = openref-mcp の絶対パス(add のとき)、$3 = いまの設定ファイルの中身(無ければ空)。
# 標準出力 = 新しい JSON。終了コード 3 = 変更なし。それ以外の失敗 = JSON が壊れている・UTF-8 でない等(呼び手はファイルに触らない)。
# add は既存の openref 項目の他のキー(env 等)を保って command だけを向け直す(args は無ければ [])
claude_desktop_config_edit() {
  OPENREF_CFG_TEXT="$3" osascript -l JavaScript -e '
    ObjC.import("stdlib");
    function run(argv) {
      const mode = argv[0], bin = argv[1];
      const raw = $.getenv("OPENREF_CFG_TEXT");
      // 環境変数は常に渡している。undefined = 中身が UTF-8 として読めない → 空とみなして作り直すと他の登録が消えるので失敗にする
      if (raw === undefined || raw === null) throw new Error("config is not UTF-8");
      const text = String(raw);
      let d = {};
      if (text.trim() !== "") d = JSON.parse(text);
      if (typeof d !== "object" || d === null || Array.isArray(d)) throw new Error("JSON object expected");
      const isObj = function (v) { return typeof v === "object" && v !== null && !Array.isArray(v); };
      let servers = isObj(d.mcpServers) ? d.mcpServers : {};
      const before = JSON.stringify(servers.openref === undefined ? null : servers.openref);
      if (mode === "add") {
        const next = Object.assign({}, isObj(servers.openref) ? servers.openref : {}, { command: bin });
        if (!Array.isArray(next.args)) next.args = [];
        servers.openref = next;
      } else {
        delete servers.openref;
      }
      const after = JSON.stringify(servers.openref === undefined ? null : servers.openref);
      if (before === after && text.trim() !== "") $.exit(3);
      d.mcpServers = servers;
      return JSON.stringify(d, null, 2);
    }' -- "$1" "$2" 2>/dev/null
}
# $1 = add | remove、$2 = openref-mcp の絶対パス(add のとき)。設定ファイルを読んで、必要なら書き戻す。
# 戻り値 0 = 書いた、3 = 変更なし、1 = 失敗(理由を CLAUDE_CFG_ERR に入れる。呼び手が案内し、処理は止めない)
claude_desktop_config_apply() {
  CDC_CFG="$HOME/Library/Application Support/Claude/claude_desktop_config.json"
  CLAUDE_CFG_ERR=""
  if [ -L "$CDC_CFG" ]; then CLAUDE_CFG_ERR="設定ファイルがシンボリックリンクなので触りません"; return 1; fi
  CDC_CUR=""
  if [ -e "$CDC_CFG" ]; then
    if ! CDC_CUR="$(cat "$CDC_CFG" 2>/dev/null)"; then CLAUDE_CFG_ERR="設定ファイルを読めません"; return 1; fi
    # NUL を含むファイルはシェルの読み出しで NUL が落ちて別の内容になる(正しい JSON ではないはず)→ 1 つでも含めば触らない
    if ! LC_ALL=C tr -d '\000' < "$CDC_CFG" 2>/dev/null | cmp -s - "$CDC_CFG"; then
      CLAUDE_CFG_ERR="設定ファイルを文字列として読めません(NUL を含む)"; return 1
    fi
  fi
  CDC_NEW=""
  if CDC_NEW="$(claude_desktop_config_edit "$1" "$2" "$CDC_CUR")"; then :; else
    CDC_RC=$?
    if [ "$CDC_RC" = "3" ]; then return 3; fi
    CLAUDE_CFG_ERR="設定ファイルが正しい JSON(UTF-8)として読めません(大きすぎる場合も)"; return 1
  fi
  # いまの内容を同じフォルダへ退避(= 今回の書き込みを戻すためのもの。実際に書き換えるときだけ、そのたびに作り直す)。退避できなければ書かない
  if [ -e "$CDC_CFG" ]; then
    rm -f "$CDC_CFG.bak-openref" 2>/dev/null || true
    if ! cp -p "$CDC_CFG" "$CDC_CFG.bak-openref" 2>/dev/null; then CLAUDE_CFG_ERR="設定ファイルを退避できません"; return 1; fi
  fi
  # 所有者だけが読める新しいファイル(mktemp = 0600 で新規作成。残り物やシンボリックリンクを再利用しない)に隣で書いてから差し替える
  # (他の MCP の env に秘密が入っていることがある)
  CDC_TMP="$(mktemp "$CDC_CFG.tmp-openref.XXXXXX" 2>/dev/null)" || { CLAUDE_CFG_ERR="設定ファイルに書き込めません"; return 1; }
  if printf '%s\n' "$CDC_NEW" > "$CDC_TMP" 2>/dev/null && mv -f "$CDC_TMP" "$CDC_CFG" 2>/dev/null; then return 0; fi
  rm -f "$CDC_TMP" 2>/dev/null || true
  CLAUDE_CFG_ERR="設定ファイルに書き込めません"
  return 1
}
# ---- Claude Desktop(チャット)の MCP 登録: ここまで ----
unregister_claude_desktop() {
  CFG="$HOME/Library/Application Support/Claude/claude_desktop_config.json"
  [ -e "$CFG" ] || return 0
  case "$(cat "$CFG" 2>/dev/null || true)" in *openref*) ;; *) return 0 ;; esac
  if claude_desktop_running; then
    echo "注意: Claude Desktop が起動中(または起動の確認ができない)ため、設定から MCP(openref)を外せませんでした。Claude Desktop を終了(⌘Q)してからもう一度このアンインストーラを実行すると外れます(そのままだと、次に Claude Desktop を開いたとき openref の起動失敗が表示されます)"
    return 0
  fi
  if claude_desktop_config_apply remove ""; then
    echo "==> Claude Desktop の設定から MCP(openref)を外しました"
  elif [ "$?" != "3" ]; then
    echo "注意: Claude Desktop の設定($CFG)には手を付けませんでした: ${CLAUDE_CFG_ERR}。mcpServers.openref は手で削除してください"
  fi
  return 0
}

main() {
  SUPPORT="$HOME/Library/Application Support/OpenRef"
  OLD_SUPPORT="$HOME/Library/Application Support/Refract"
  WEF="$HOME/Library/Containers/com.microsoft.Word/Data/Documents/wef"
  OFFICE_GC="$HOME/Library/Group Containers/UBF8T346G9.Office"

  echo "==> 常駐サービスを停止・解除します"
  # dev.refract.connectd = 改名前の名前
  for label in dev.openref.connectd dev.refract.connectd; do
    launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
    rm -f "$HOME/Library/LaunchAgents/$label.plist"
  done
  rm -f "$SUPPORT/bin/openref" "$SUPPORT/bin/openref-mcp" "$SUPPORT/bin/openref-connectd" \
        "$SUPPORT/bin/refract" "$SUPPORT/bin/refract-mcp" "$SUPPORT/bin/refract-connectd" \
        "$OLD_SUPPORT/bin/refract" "$OLD_SUPPORT/bin/refract-mcp" "$OLD_SUPPORT/bin/refract-connectd"

  echo "==> アプリと拡張を削除します"
  pkill -x openref-app 2>/dev/null || true
  pkill -x refract-app 2>/dev/null || true
  rm -rf "/Applications/OpenRef.app" "$HOME/Applications/OpenRef.app"
  rm -rf "/Applications/Refract.app" "$HOME/Applications/Refract.app"
  rm -rf "$HOME/Applications/OpenRef Capture" "$HOME/Applications/Refract Capture"
  rm -f "$WEF/openref-manifest.xml" "$WEF/refract-manifest.xml"
  # Word を再起動すると OpenRef タブが消える(起動中の Word からは終了するまで消えない)
  rm -f "$OFFICE_GC/User Content.localized/Startup.localized/Word/OpenRef.dotm" "$OFFICE_GC/User Content/Startup/Word/OpenRef.dotm"
  rm -rf "$SUPPORT/word/OpenRef Dialog.app"

  unregister_claude_desktop

  KEPT="$SUPPORT"
  if [ ! -d "$SUPPORT" ] && [ -d "$OLD_SUPPORT" ]; then
    KEPT="$OLD_SUPPORT"
  fi
  cat <<EOF

✅ アンインストール完了。

ライブラリデータ(登録した論文・PDF・注釈)は残してあります:
  $KEPT
完全に削除する場合は、このフォルダを Finder で手動削除してください。
EOF
}

main "$@"
