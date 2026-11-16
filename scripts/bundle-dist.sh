#!/bin/bash
# 配布物(openref-macos-arm64.tar.gz)を作る: 版の検査 → release build → 組み立て → sha256。
# .github/workflows/release.yml(GitHub Release)が呼ぶ。手元で同じ物を作るときもこれを使う。
# 使い方:
#   scripts/bundle-dist.sh [--check-only | --no-build] <stage dir> <X.Y.Z>
#     <stage dir> は target/ の下の、名前が -dist で終わるフォルダだけ(例: target/release-dist)。中身は毎回消して作り直す
#     --check-only  版の検査だけして終わる(build も組み立てもしない)
#     --no-build    build を飛ばし、target/release にある物で組み立てる
# 出し物: <stage>/dl/openref-macos-arm64.tar.gz・.tar.gz.sha256・VERSION(tar の VERSION と同じ 1 行 = app の新版確認が読む)
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

fail() {
  if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::error::$*"; fi
  echo "エラー: $*" >&2
  exit 1
}
USAGE="usage: scripts/bundle-dist.sh [--check-only | --no-build] <stage dir> <X.Y.Z>"

MODE=all
case "${1:-}" in
  --check-only) MODE=check; shift ;;
  --no-build) MODE=nobuild; shift ;;
esac
# 引数はちょうど 2 つ(余った引数・後ろに置いた --no-build を黙って無視しない)
if [ $# -ne 2 ]; then fail "引数は <stage dir> <X.Y.Z> の 2 つ($# 個あった)。$USAGE"; fi
STAGE="$1"
VER="$2"
# stage は毎回 rm -rf する = target/<名前>-dist の形だけ許す(target そのもの・target/release・絶対パス・.. などは拒む)
stage_ok() { [[ "$1" =~ ^target/[A-Za-z0-9][A-Za-z0-9_-]*-dist$ ]]; }
if ! stage_ok "$STAGE"; then fail "stage は target/<名前>-dist の形だけ: '$STAGE'"; fi
# symlink の stage と target も初めに止める(build の後で止まると build が無駄になる。消す直前にもう一度見る)
if [ -L "$STAGE" ] || [ -L target ]; then fail "stage と target は symlink にしない: '$STAGE'"; fi

# 1. 版の検査。版の出どころは Cargo.toml の [workspace.package] version 1 つ(tauri.conf.json に version を置かない =
#    Tauri が CARGO_PKG_VERSION を Info.plist と package_info に使う)。app/package.json も同じ値で揃える
# 版は X.Y.Z の 3 つ組だけ(app の新版確認 = core update.rs は pre-release / build metadata を読めず、配布後の全クライアントで確認が失敗する)
if ! [[ "$VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  fail "版は X.Y.Z の形だけ(dl/VERSION を app が読めない): $VER"
fi
CARGO_VER="$(python3 -c 'import tomllib, pathlib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["workspace"]["package"]["version"])')"
PKG_VER="$(python3 -c 'import json, pathlib; print(json.loads(pathlib.Path("app/package.json").read_text())["version"])')"
echo "版: $VER / Cargo.toml $CARGO_VER / app/package.json $PKG_VER"
if [ "$VER" != "$CARGO_VER" ] || [ "$VER" != "$PKG_VER" ]; then
  fail "版の不一致: $VER / Cargo.toml($CARGO_VER) / app/package.json($PKG_VER)"
fi
if [ "$MODE" = check ]; then exit 0; fi

# 2. build(--locked = Cargo.lock を書き換えない。古ければ止まる)
if [ "$MODE" = all ]; then
  cargo build --release --locked -p openref-cli -p openref-mcp -p openref-connectd
  (cd app && npm run tauri build -- --bundles app)
fi

# 3. 組み立て
# 消す直前にもう一度: repo の target/ の直下の -dist のフォルダであること(symlink は辿らせない)
if ! stage_ok "$STAGE" || [ -L "$STAGE" ] || [ -L target ]; then fail "stage を消さない: '$STAGE'"; fi
rm -rf "$STAGE"
mkdir -p "$STAGE/bundle/bin" "$STAGE/bundle/word" "$STAGE/dl"
printf '%s\n' "$VER" > "$STAGE/bundle/VERSION"
cp -R target/release/bundle/macos/OpenRef.app "$STAGE/bundle/OpenRef.app"
# 安全網: build した .app の版が配る版と同じこと
PLIST_VER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$STAGE/bundle/OpenRef.app/Contents/Info.plist")"
if [ "$PLIST_VER" != "$VER" ]; then
  fail "Info.plist CFBundleShortVersionString($PLIST_VER) != $VER"
fi
for bin in openref openref-mcp openref-connectd; do
  cp "target/release/$bin" "$STAGE/bundle/bin/$bin"
  codesign --force -s - "$STAGE/bundle/bin/$bin"
  codesign -v "$STAGE/bundle/bin/$bin"
done
# 拡張の試験(extension/tests)は配布しない = 利用者に要らない(公開 repo には残る = CI が回す)
rsync -a --exclude ".DS_Store" --exclude "/tests" extension/ "$STAGE/bundle/extension/"
scripts/bundle-word.sh "$STAGE/bundle/word"  # OpenRef.dotm + OpenRef Dialog.app
# Tauri は bundle 全体を署名しないため ad-hoc で締め直す(無署名配布でも codesign -v を通す)
codesign --force --deep -s - "$STAGE/bundle/OpenRef.app"
codesign -v "$STAGE/bundle/OpenRef.app"
tar -czf "$STAGE/dl/openref-macos-arm64.tar.gz" -C "$STAGE/bundle" VERSION OpenRef.app bin extension word

# 4. sha256 と版番号ファイル(app の起動時の新版確認が読む = core update.rs の CHECK_URL = /dl/VERSION)
(cd "$STAGE/dl" && shasum -a 256 openref-macos-arm64.tar.gz > openref-macos-arm64.tar.gz.sha256)
printf '%s\n' "$VER" > "$STAGE/dl/VERSION"
echo "✓ $STAGE/dl/openref-macos-arm64.tar.gz (v$VER)"
