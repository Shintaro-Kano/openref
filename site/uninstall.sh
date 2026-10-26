#!/bin/bash
# OpenRef アンインストーラ — https://openref.unitedradiology.jp
# アプリ・常駐サービス・拡張・Word 連携(Startup の OpenRef.dotm・検索画面・旧 Office アドイン manifest)を削除します
# (改名前の Refract の名前で入っているものも)。
# ライブラリデータ(~/Library/Application Support/OpenRef。旧版のままなら …/Refract)は消しません。
set -euo pipefail

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
