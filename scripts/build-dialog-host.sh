#!/bin/bash
# OpenRef Dialog.app（Word の Startup テンプレート版 OpenRef.dotm のダイアログの器）を作る。
# word/dialog-host/host.js（JXA）→ osacompile でアプレット → Info.plist に LSUIElement（Dock・メニューバーに出さない）→ ad-hoc 再署名。
# LaunchServices（open -a）で起動したときだけ窓が前面・キー窓になる（素の osascript では macOS 14+ で前面化されない）。
# 使い方: scripts/build-dialog-host.sh <出力ディレクトリ>   → <出力ディレクトリ>/OpenRef Dialog.app
# 置き場所は ~/Library/Application Support/OpenRef/word/（connectd が起動する。OPENREF_DIALOG_APP で差し替え可）
set -euo pipefail

OUT="${1:?使い方: $0 <出力ディレクトリ>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/word/dialog-host/host.js"
APP="$OUT/OpenRef Dialog.app"

mkdir -p "$OUT"
rm -rf "$APP"
osacompile -l JavaScript -o "$APP" "$SRC"
PLIST="$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Delete :LSUIElement" "$PLIST" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :LSUIElement bool true" "$PLIST"
# Info.plist を書き換えたので署名し直す（osacompile の署名は無効になっている）
codesign --force --sign - "$APP"
codesign --verify "$APP"
echo "$APP"
