#!/bin/bash
# OpenRef インストーラ — https://openref.unitedradiology.jp
#
# このスクリプトがやること(全リスト):
#   0. 旧名(Refract)の版が入っていれば片付ける: 常駐サービスを止めて登録を外し、Refract.app・拡張フォルダ・
#      Word manifest を削除。ライブラリ(~/Library/Application Support/Refract)は消さずに OpenRef へ引き継ぐ
#   1. OpenRef.app を /Applications へ設置(書込不可なら ~/Applications)
#   2. 取込サービス openref-connectd・CLI(openref)・MCP(openref-mcp) を ~/Library/Application Support/OpenRef/bin へ設置
#   3. LaunchAgent (dev.openref.connectd) を登録し常駐開始(127.0.0.1 のみで待機)
#   4. Chrome 拡張フォルダを ~/Applications/OpenRef Capture へ設置
#   5. Word がある場合: Word の Startup フォルダに OpenRef.dotm(リボンの OpenRef タブ)、引用の検索画面 OpenRef Dialog.app を
#      ~/Library/Application Support/OpenRef/word へ設置し、ローカル証明書を信頼登録
#      (このとき Mac のパスワード確認が 1 回出ることがあります)。旧方式の Office アドイン manifest は外す
#
# 更新: 同じコマンドをもう一度実行するだけ
# 削除: curl -fsSL https://openref.unitedradiology.jp/uninstall.sh | bash
set -euo pipefail

main() {
  HOME="${HOME%/}"  # 末尾の / を落とす(DB に入っている絶対パスと前方一致で比べるため)
  BASE_URL="${OPENREF_BASE_URL:-https://openref.unitedradiology.jp}"
  SUPPORT="$HOME/Library/Application Support/OpenRef"
  APP_DIR="/Applications"
  AGENT_LABEL="dev.openref.connectd"
  AGENT_PLIST="$HOME/Library/LaunchAgents/$AGENT_LABEL.plist"
  CAPTURE_DIR="$HOME/Applications/OpenRef Capture"
  WORD_CONTAINER="$HOME/Library/Containers/com.microsoft.Word"
  WEF="$WORD_CONTAINER/Data/Documents/wef"
  OFFICE_GC="$HOME/Library/Group Containers/UBF8T346G9.Office"
  # 改名前(Refract)の名前 — 旧版からの更新で片付ける
  OLD_SUPPORT="$HOME/Library/Application Support/Refract"
  OLD_LABEL="dev.refract.connectd"
  OLD_PLIST="$HOME/Library/LaunchAgents/$OLD_LABEL.plist"

  msg()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
  warn() { printf '\033[1;33m注意:\033[0m %s\n' "$*"; }

  # ---- 環境チェック ----
  if [ "$(uname)" != "Darwin" ]; then
    echo "このインストーラは Mac 専用です。"; exit 1
  fi
  if [ "$(uname -m)" != "arm64" ]; then
    echo "この Mac(Intel 版)には現在未対応です。配布元にお知らせください。"; exit 1
  fi
  osmajor="$(sw_vers -productVersion | cut -d. -f1)"
  if [ "$osmajor" -lt 12 ]; then
    warn "macOS $(sw_vers -productVersion) では動作未確認です(推奨: macOS 14 以降)"
  fi

  # ---- ダウンロード ----
  # OPENREF_BASE_URL が GitHub Releases (.../releases/latest/download または .../releases/download/vX.Y.Z) の場合は直下の tar.gz、
  # 通常のサイトルート (https://openref.unitedradiology.jp) の場合は /dl/openref-macos-arm64.tar.gz を引く
  BASE_URL="${BASE_URL%/}"
  case "$BASE_URL" in
    */releases/latest/download|*/releases/download/*)
      TARBALL_URL="${OPENREF_TARBALL_URL:-$BASE_URL/openref-macos-arm64.tar.gz}" ;;
    *)
      TARBALL_URL="${OPENREF_TARBALL_URL:-$BASE_URL/dl/openref-macos-arm64.tar.gz}" ;;
  esac
  msg "OpenRef をダウンロードしています…"
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  curl -fsSL "$TARBALL_URL" | tar -xz -C "$TMP"
  if [ -f "$TMP/VERSION" ]; then
    msg "OpenRef v$(cat "$TMP/VERSION") を展開しました"
  fi

  # ---- 旧名(Refract)の版からの更新 ----
  FROM_REFRACT=""
  if [ -e "$OLD_PLIST" ] || [ -d "$OLD_SUPPORT" ] || [ -d "/Applications/Refract.app" ] || [ -d "$HOME/Applications/Refract.app" ] \
     || [ -d "$HOME/Applications/Refract Capture" ] || [ -e "$WEF/refract-manifest.xml" ]; then
    FROM_REFRACT=1
    msg "旧版(Refract)を止めています…(ライブラリは引き継ぎます)"
    launchctl bootout "gui/$(id -u)/$OLD_LABEL" 2>/dev/null || true
    pkill -x refract-app 2>/dev/null || true
    # 旧ライブラリを開いたままのプロセスがあると引き継ぎが止まるので、終わるのを待つ
    for _ in $(seq 1 20); do
      pgrep -x refract-connectd >/dev/null 2>&1 || pgrep -x refract-app >/dev/null 2>&1 || break
      sleep 0.5
    done
  fi
  # ライブラリの引き継ぎ(…/Refract → …/OpenRef)。Rust 側に自動移行は無く、ここで行う。
  # 旧版は上で止めてある。① ディレクトリごと rename(コピーしない)→ ② 旧 DB の写しを backup/ に取り、managed 添付の絶対パスを
  # 書き換え、-wal / -shm・cert → 最後に DB 本体を改名(旧名の DB 本体が残っている = 未完の印。新版はこの印があると空の蔵書を
  # 作らずに止まる)。② は旧名の DB 本体が残っていれば何度でも走る(途中で止まっても再実行で続きから。UPDATE と改名は冪等)。
  # 取込サービスの置き場(…/OpenRef/bin)を作ると移行先が空でなくなるので、その前に済ませる
  if [ -d "$OLD_SUPPORT" ] && [ ! -e "$SUPPORT" ]; then
    msg "ライブラリを引き継いでいます(Refract → OpenRef)…"
    if pgrep -x refract-connectd >/dev/null 2>&1 || pgrep -x refract-app >/dev/null 2>&1; then
      # まだ何も動かしていない: 旧版の常駐サービスを戻してから止める(どちらの版も無い状態にしない)
      if [ -e "$OLD_PLIST" ]; then launchctl bootstrap "gui/$(id -u)" "$OLD_PLIST" 2>/dev/null || true; fi
      warn "旧版(Refract)がまだ動いています。終了してから、もう一度インストーラを実行してください"; exit 1
    fi
    mv "$OLD_SUPPORT" "$SUPPORT"
  fi
  if [ -e "$SUPPORT/refract.sqlite" ] && [ ! -e "$SUPPORT/openref.sqlite" ]; then
    msg "ライブラリの名前を変えています(refract → openref)…"
    mkdir -p "$SUPPORT/backup"
    sqlite3 "$SUPPORT/refract.sqlite" ".backup '$SUPPORT/backup/pre-rename-$(date -u +%Y%m%dT%H%M%SZ).sqlite'"
    # SQL の文字列は ' を '' に。前方一致は substr の完全一致(LIKE の _ % と大小無視を避ける)
    OLD_SQL="$(printf '%s' "$OLD_SUPPORT" | sed "s/'/''/g")"; NEW_SQL="$(printf '%s' "$SUPPORT" | sed "s/'/''/g")"
    sqlite3 "$SUPPORT/refract.sqlite" "UPDATE attachments SET path = '$NEW_SQL/' || substr(path, length('$OLD_SQL/') + 1) WHERE substr(path, 1, length('$OLD_SQL/')) = '$OLD_SQL/'"
    left="$(sqlite3 "$SUPPORT/refract.sqlite" "SELECT count(*) FROM attachments WHERE mode = 'managed' AND substr(path, 1, length('$NEW_SQL/')) <> '$NEW_SQL/'")"
    if [ "$left" != "0" ]; then
      warn "取り込み済み PDF $left 件のパスが新しい場所を指していません(起動後に PDF が開けなければ配布元にお知らせください)"
    fi
    rename_if_present() { if [ -e "$SUPPORT/$1" ]; then mv "$SUPPORT/$1" "$SUPPORT/$2"; fi; }
    rename_if_present refract.sqlite-wal openref.sqlite-wal
    rename_if_present refract.sqlite-shm openref.sqlite-shm
    rename_if_present certs/refract-localhost.pem certs/openref-localhost.pem
    rename_if_present certs/refract-localhost.key.pem certs/openref-localhost.key.pem
    mv "$SUPPORT/refract.sqlite" "$SUPPORT/openref.sqlite"
  fi
  # 旧版の片付けは引き継ぎが済んでから(旧 connectd の本体は引き継ぎで …/OpenRef/bin へ移っていることがある)
  if [ -n "$FROM_REFRACT" ]; then
    msg "旧版(Refract)を片付けています…"
    rm -f "$OLD_PLIST"
    rm -rf "/Applications/Refract.app" "$HOME/Applications/Refract.app"
    rm -rf "$HOME/Applications/Refract Capture"
    rm -f "$WEF/refract-manifest.xml"
    rm -f "$SUPPORT/bin/refract" "$SUPPORT/bin/refract-mcp" "$SUPPORT/bin/refract-connectd" \
          "$OLD_SUPPORT/bin/refract" "$OLD_SUPPORT/bin/refract-mcp" "$OLD_SUPPORT/bin/refract-connectd"
  fi

  # ---- アプリ設置 ----
  msg "OpenRef.app を設置しています…"
  if [ ! -w "$APP_DIR" ]; then
    APP_DIR="$HOME/Applications"
    mkdir -p "$APP_DIR"
  fi
  pkill -x openref-app 2>/dev/null || true
  rm -rf "$APP_DIR/OpenRef.app"
  cp -R "$TMP/OpenRef.app" "$APP_DIR/OpenRef.app"

  # ---- 取込サービス(connectd)・CLI・MCP ----
  msg "取込サービス・CLI・MCP を設置しています…"
  # 旧プロセスを先に停止(未登録なら何もしない)
  launchctl bootout "gui/$(id -u)/$AGENT_LABEL" 2>/dev/null || true
  mkdir -p "$SUPPORT/bin"
  for bin in openref openref-mcp openref-connectd; do
    if [ -f "$TMP/bin/$bin" ]; then
      rm -f "$SUPPORT/bin/$bin"
      cp "$TMP/bin/$bin" "$SUPPORT/bin/$bin"
      chmod +x "$SUPPORT/bin/$bin"
    fi
  done

  mkdir -p "$HOME/Library/LaunchAgents"
  cat > "$AGENT_PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$AGENT_LABEL</string>
  <key>ProgramArguments</key>
  <array><string>$SUPPORT/bin/openref-connectd</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>StandardErrorPath</key><string>/tmp/openref-connectd.log</string>
</dict>
</plist>
PLIST
  launchctl bootstrap "gui/$(id -u)" "$AGENT_PLIST"

  # 起動確認(:23119 ping)
  ok=""
  for _ in $(seq 1 30); do
    if curl -fsS -m 1 "http://127.0.0.1:23119/connector/ping" >/dev/null 2>&1; then
      ok=1; break
    fi
    sleep 0.5
  done
  if [ -z "$ok" ]; then
    warn "取込サービスの起動が確認できませんでした。Zotero を起動中なら終了して再実行してください(ログ: /tmp/openref-connectd.log)"
  fi

  # ---- Chrome 拡張 ----
  msg "Chrome 拡張(OpenRef Capture)を設置しています…"
  mkdir -p "$HOME/Applications"
  rm -rf "$CAPTURE_DIR"
  cp -R "$TMP/extension" "$CAPTURE_DIR"

  # ---- Word(リボンの OpenRef タブ)----
  # Word の Startup フォルダのテンプレート OpenRef.dotm = Word を終了・再起動してもタブが消えない(Office アドイン方式だと
  # Word 16.114 以降、再起動のたびに「ホーム > アドイン」から選び直しが要る)。引用の検索画面は OpenRef Dialog.app
  if [ -d "$WORD_CONTAINER" ] || [ -d "/Applications/Microsoft Word.app" ]; then
    msg "Word 連携(リボンの OpenRef タブ)を設置しています…"
    if [ -d "$OFFICE_GC/User Content.localized" ] || [ ! -d "$OFFICE_GC/User Content" ]; then
      STARTUP="$OFFICE_GC/User Content.localized/Startup.localized/Word"
    else
      STARTUP="$OFFICE_GC/User Content/Startup/Word"
    fi
    mkdir -p "$STARTUP" "$SUPPORT/word"
    cp "$TMP/word/OpenRef.dotm" "$STARTUP/.OpenRef.dotm.new"
    mv -f "$STARTUP/.OpenRef.dotm.new" "$STARTUP/OpenRef.dotm"
    rm -rf "$SUPPORT/word/OpenRef Dialog.app"
    cp -R "$TMP/word/OpenRef Dialog.app" "$SUPPORT/word/OpenRef Dialog.app"
    # 旧方式の Office アドイン manifest(残すと「ホーム > アドイン」から選んだときにタブが二重になる)
    rm -f "$WEF/openref-manifest.xml"
    CERT="$SUPPORT/certs/openref-localhost.pem"
    for _ in $(seq 1 20); do
      [ -f "$CERT" ] && break
      sleep 0.5
    done
    if [ -f "$CERT" ]; then
      msg "引用機能用のローカル証明書を信頼登録します(Mac のパスワード確認が出たら入力してください)"
      if ! security add-trusted-cert -r trustRoot \
           -k "$HOME/Library/Keychains/login.keychain-db" "$CERT"; then
        warn "証明書の信頼登録をスキップしました。Word 引用機能を使うときにもう一度インストーラを実行してください。"
      fi
    else
      warn "証明書が未生成のため Word の信頼登録をスキップしました。もう一度インストーラを実行すると完了します。"
    fi
    # ps の出力を変数に取ってから見る(pipefail の下で grep -q が先に終わると ps が SIGPIPE で落ちて判定を誤る)
    PROCS="$(ps -axo comm= 2>/dev/null || true)"
    case "$PROCS" in
      *"/Microsoft Word"*) warn "Word が起動中です。Word を一度終了(⌘Q)して開き直すと OpenRef タブが出ます" ;;
    esac
  else
    msg "Word が見つからないため、Word 連携はスキップしました(Word 導入後に再実行すれば入ります)"
  fi

  # ---- 完了 ----
  msg "OpenRef を起動します"
  open "$APP_DIR/OpenRef.app"
  cat <<'DONE'

✅ インストール完了!

つぎのステップ:
  • Chrome 拡張の有効化(3 ステップ):
      https://openref.unitedradiology.jp/#capture
  • Word で使う: Word を開き直す(起動中なら ⌘Q で終了してから)→
      リボンの「参照設定」の右に OpenRef タブ(以後は毎回自動で出ます)

更新するときは、同じコマンドをもう一度実行するだけです。
DONE
  if [ -n "$FROM_REFRACT" ]; then
    cat <<'RENAMED'
旧版(Refract)から更新しました:
  • Chrome の拡張機能ページ(chrome://extensions)で旧「Refract Capture」を削除し、
    「OpenRef Capture」フォルダを読み込み直してください
  • Word は開き直すとリボンに OpenRef タブが出ます(文書の引用はそのまま使えます)
RENAMED
  fi
}

main "$@"
