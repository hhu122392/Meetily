#!/usr/bin/env bash
set -euo pipefail
TARGET="${1:?需要指定 Mac 架构}"
case "$TARGET" in
  aarch64-apple-darwin|x86_64-apple-darwin) ;;
  *) echo '不支持的 Mac 架构' >&2; exit 1 ;;
esac
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export CARGO_TARGET_DIR="$ROOT/target"
export CMAKE_PROJECT_INCLUDE="$ROOT/.github/force-portable-ggml.cmake"
MANIFEST="$(mktemp)"
trap 'rm -f "$MANIFEST"' EXIT
# 只编译一次测试程序，再运行各组测试，避免重复执行 Tauri 的 sidecar 复制和链接。
if ! cargo test --locked --release -p meetily --lib --target "$TARGET" --no-run --message-format=json > "$MANIFEST"; then
  cat "$MANIFEST"
  exit 1
fi
TEST_BINARY="$(node -e '
const fs = require("fs");
const rows = fs.readFileSync(process.argv[1], "utf8").split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line));
const paths = [...new Set(rows.filter(row => row.reason === "compiler-artifact" && row.target?.name === "app_lib" && row.profile?.test && row.executable).map(row => row.executable))];
if (paths.length !== 1 || !fs.existsSync(paths[0])) throw new Error("无法唯一定位 Meetily 测试程序");
process.stdout.write(paths[0]);
' "$MANIFEST")"
export CARGO_MANIFEST_DIR="$ROOT/frontend/src-tauri"
cd "$CARGO_MANIFEST_DIR"
for FILTER in \
  summary::action_extraction::tests \
  lark::actions::tests \
  lark::macos_credentials::tests \
  parakeet_engine::parakeet_engine::tests \
  sensevoice_engine::model::tests \
  onboarding::tests; do
  LISTING="$("$TEST_BINARY" "$FILTER" --list)"
  [[ "$LISTING" == *": test"* ]] || { echo "测试组没有匹配的测试：$FILTER" >&2; exit 1; }
  "$TEST_BINARY" "$FILTER"
done
cd "$ROOT"
BUNDLE="target/$TARGET/release/bundle"
test -f "$BUNDLE/macos/meetily.app/Contents/MacOS/meetily"
codesign --verify --deep --strict "$BUNDLE/macos/meetily.app"
DMGS=("$BUNDLE"/dmg/*.dmg)
test -f "${DMGS[0]}"
for DMG in "${DMGS[@]}"; do
  hdiutil verify "$DMG"
done
