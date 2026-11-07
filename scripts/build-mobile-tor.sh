#!/usr/bin/env bash
# Build crates/mobile-tor (embedded Arti) for the phone app.
#
#   scripts/build-mobile-tor.sh ios [aarch64-apple-ios aarch64-apple-ios-sim]
#     -> apps/mobile/modules/ob-tor/ios/ObMobileTor.xcframework
#   scripts/build-mobile-tor.sh android [arm64-v8a x86_64]
#     -> apps/mobile/modules/ob-tor/android/src/main/jniLibs/<abi>/libob_mobile_tor.so
#
# Android needs ANDROID_NDK_HOME (or ANDROID_HOME with ndk/27.1.12297006).
# Set OB_MOBILE_TOR_CLEAN=1 to delete each target's build dir once its library is copied out.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
module="$root/apps/mobile/modules/ob-tor"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
export CARGO_PROFILE_RELEASE_DEBUG=0
export CARGO_PROFILE_RELEASE_STRIP="${CARGO_PROFILE_RELEASE_STRIP:-debuginfo}"

build() { # target crate-type
  rustup target add "$1" >/dev/null
  cargo rustc --manifest-path "$root/Cargo.toml" -p ob-mobile-tor --lib --release --locked --target "$1" --crate-type "$2"
}

clean_target() {
  if [ "${OB_MOBILE_TOR_CLEAN:-0}" = 1 ]; then rm -rf "${target_dir:?}/$1" || rm -rf "${target_dir:?}/$1" || true; fi
}

ios() {
  local targets=("$@")
  [ ${#targets[@]} -gt 0 ] || targets=(aarch64-apple-ios aarch64-apple-ios-sim)
  export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-16.4}"
  local work headers args=()
  work="$(mktemp -d)"
  headers="$work/include"
  mkdir -p "$headers"
  cp "$root/crates/mobile-tor/include/ob_mobile_tor.h" "$headers/"
  printf 'module ObMobileTor {\n  header "ob_mobile_tor.h"\n  export *\n}\n' > "$headers/module.modulemap"
  for target in "${targets[@]}"; do
    build "$target" staticlib
    mkdir -p "$work/$target"
    cp "$target_dir/$target/release/libob_mobile_tor.a" "$work/$target/"
    args+=(-library "$work/$target/libob_mobile_tor.a" -headers "$headers")
    clean_target "$target"
  done
  rm -rf "$module/ios/ObMobileTor.xcframework"
  xcodebuild -create-xcframework "${args[@]}" -output "$module/ios/ObMobileTor.xcframework"
  rm -rf "$work"
}

android() {
  local abis=("$@")
  [ ${#abis[@]} -gt 0 ] || abis=(arm64-v8a x86_64)
  local ndk="${ANDROID_NDK_HOME:-${ANDROID_HOME:?set ANDROID_HOME or ANDROID_NDK_HOME}/ndk/27.1.12297006}"
  local host
  case "$(uname -s)" in
    Darwin) host=darwin-x86_64 ;;
    *) host=linux-x86_64 ;;
  esac
  local bin="$ndk/toolchains/llvm/prebuilt/$host/bin" api=24
  export CARGO_PROFILE_RELEASE_STRIP=symbols
  for abi in "${abis[@]}"; do
    local target
    case "$abi" in
      arm64-v8a) target=aarch64-linux-android ;;
      x86_64) target=x86_64-linux-android ;;
      *) echo "unknown ABI $abi" >&2; exit 2 ;;
    esac
    local env_target upper
    env_target="${target//-/_}"
    upper="$(echo "$env_target" | tr '[:lower:]' '[:upper:]')"
    export "CC_$env_target=$bin/$target$api-clang"
    export "AR_$env_target=$bin/llvm-ar"
    export "RANLIB_$env_target=$bin/llvm-ranlib"
    export "CARGO_TARGET_${upper}_LINKER=$bin/$target$api-clang"
    export "CARGO_TARGET_${upper}_RUSTFLAGS=-C link-arg=-Wl,-z,max-page-size=16384"
    build "$target" cdylib
    mkdir -p "$module/android/src/main/jniLibs/$abi"
    cp "$target_dir/$target/release/libob_mobile_tor.so" "$module/android/src/main/jniLibs/$abi/"
    clean_target "$target"
  done
}

case "${1:-}" in
  ios) shift; ios "$@" ;;
  android) shift; android "$@" ;;
  *) echo "usage: $0 ios|android [targets...]" >&2; exit 2 ;;
esac
