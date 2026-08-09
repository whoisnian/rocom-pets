#!/usr/bin/env bash
#
# build-wasm.sh — 出下载站预览用的那份 wasm(见 src/web.rs)
#
# 编的是**库**(`--lib`):`src/main.rs` 那个 bin 依赖窗口/托盘,wasm 上不存在,
# 不加 `--lib` 的话 cargo 会连它一起编然后报一堆找不到模块。
#
# 产物落在 web/src/wasm/,由 vite 打包(动态 import ⇒ 单独一个 chunk,
# 只有点开预览弹窗的人才会下)。那个目录是生成物,不入仓库。
#
# 依赖:rustup target add wasm32-unknown-unknown
#       cargo install wasm-bindgen-cli --version <与 Cargo.lock 里的 wasm-bindgen 一致>
#       版本对不上 wasm-bindgen 会当场告诉你该装哪个。
#
#   scripts/build-wasm.sh          # release(默认,给部署用)
#   scripts/build-wasm.sh --debug  # 快一点,带符号,报错能看
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$REPO/web/src/wasm"
PROFILE=release
[[ "${1:-}" == "--debug" ]] && PROFILE=debug

# cargo 装的东西默认在这儿,而 npm 起的子进程未必带上它
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"

command -v wasm-bindgen >/dev/null 2>&1 || {
    want=$(awk '/^name = "wasm-bindgen"$/{getline; gsub(/[^0-9.]/,""); print; exit}' "$REPO/Cargo.lock")
    echo "错误: 未找到 wasm-bindgen,请先 cargo install wasm-bindgen-cli --version ${want}" >&2
    exit 1
}

cd "$REPO"
if [[ "$PROFILE" == release ]]; then
    cargo build --lib --release --target wasm32-unknown-unknown
else
    cargo build --lib --target wasm32-unknown-unknown
fi

rm -rf "$OUT"
wasm-bindgen --target web --out-dir "$OUT" \
    "target/wasm32-unknown-unknown/$PROFILE/rocom_pets.wasm"

size=$(stat -c%s "$OUT/rocom_pets_bg.wasm")
printf '%s  %.2f MB(brotli 之后约三成)\n' "$OUT/rocom_pets_bg.wasm" "$(echo "$size" | awk '{print $1/1048576}')"
