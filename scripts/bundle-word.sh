#!/bin/bash
# Word 連携の配布物を <出力 dir> に作る: OpenRef.dotm（Startup テンプレート = リボンの OpenRef タブ）と
# OpenRef Dialog.app（引用の検索・設定・通知の画面。connectd が開く）。release.yml / deploy-site.sh / install-word-addin.sh が使う
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:?usage: scripts/bundle-word.sh <出力 dir>}"
mkdir -p "$OUT"
python3 -I "$ROOT/scripts/build-word-dotm.py" "$OUT/OpenRef.dotm"
"$ROOT/scripts/build-dialog-host.sh" "$OUT"
