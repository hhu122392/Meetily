#!/usr/bin/env bash
set -euo pipefail
if [[ "$(uname -s)" != Darwin ]]; then
  echo '此脚本需要在 macOS 上运行。' >&2
  exit 1
fi
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
case "$(uname -m)" in
  arm64) TARGET=aarch64-apple-darwin ;;
  x86_64) TARGET=x86_64-apple-darwin ;;
  *) echo '不支持此 CPU 架构。' >&2; exit 1 ;;
esac
export MACOSX_DEPLOYMENT_TARGET=14.4
export CMAKE_PROJECT_INCLUDE="$ROOT/.github/force-portable-ggml.cmake"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-3}"
export CARGO_TARGET_DIR="$ROOT/target"
for tool in node pnpm cargo cmake; do
  command -v "$tool" >/dev/null || { echo "缺少构建工具：$tool" >&2; exit 1; }
done
xcrun --find clang >/dev/null
rustup target add "$TARGET"
pnpm --dir frontend install --frozen-lockfile
cargo build --locked --release -p llama-helper --target "$TARGET" --features metal
mkdir -p frontend/src-tauri/binaries
cp "target/$TARGET/release/llama-helper" "frontend/src-tauri/binaries/llama-helper-$TARGET"
cd frontend
pnpm exec tauri build --target "$TARGET" --bundles app,dmg --config src-tauri/tauri.macos-custom.conf.json
cd "$ROOT"
find "target/$TARGET/release/bundle/dmg" -name '*.dmg' -type f
