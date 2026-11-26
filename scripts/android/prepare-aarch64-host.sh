#!/usr/bin/env bash
# Prepare an aarch64 Linux host to link the Android targets with native clang.
#
# Gradle and the Android Gradle Plugin are JVM programs and run on any host
# architecture. Only the command-line tools AGP forks are distributed as x86-64
# binaries. This script covers the Rust side of that split: it makes the NDK
# sysroot usable by the distribution's clang, so
# `cargo build --target aarch64-linux-android` links natively instead of
# through instruction emulation.
#
# It is idempotent, and a no-op on x86-64 hosts, where the NDK's own compiler
# driver works and cargo-ndk generates a matching linker wrapper.
#
# Usage: scripts/android/prepare-aarch64-host.sh [--check]
#
#   --check   Report what would change, exit non-zero if anything is missing.
#
# Requires ANDROID_NDK_HOME, or ANDROID_HOME plus the ndkVersion recorded in
# mobile/mobile-shell/gradle.properties.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

CHECK_ONLY=0
for arg in "$@"; do
    case "$arg" in
        --check) CHECK_ONLY=1 ;;
        -h|--help)
            sed -n '2,19p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "ERROR: unknown option: $arg" >&2; exit 2 ;;
    esac
done

log() { printf '%s\n' "$*"; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

host_arch="$(uname -m)"
if [[ "$host_arch" != aarch64 && "$host_arch" != arm64 ]]; then
    log "Host is $host_arch; the NDK compiler driver runs natively here. Nothing to do."
    exit 0
fi

if [[ -z "${ANDROID_NDK_HOME:-}" && -n "${ANDROID_HOME:-}" ]]; then
    ndk_version="$(sed -n 's/^ndkVersion=//p' "$REPO_ROOT/mobile/mobile-shell/gradle.properties")"
    ANDROID_NDK_HOME="$ANDROID_HOME/ndk/$ndk_version"
fi
[[ -n "${ANDROID_NDK_HOME:-}" && -d "$ANDROID_NDK_HOME" ]] || die \
    'Set ANDROID_NDK_HOME, or ANDROID_HOME plus mobile/mobile-shell/gradle.properties ndkVersion'

# The NDK publishes one host toolchain per supported host and only x86-64 exists
# for Linux. Its sysroot holds target objects, which are architecture-neutral.
NDK_PREBUILT="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64"
SYSROOT="$NDK_PREBUILT/sysroot"
[[ -d "$SYSROOT" ]] || die "Missing NDK sysroot: $SYSROOT"

clang_bin=""
for candidate in clang clang-21 clang-20 clang-19 clang-18; do
    if command -v "$candidate" >/dev/null 2>&1; then clang_bin="$(command -v "$candidate")"; break; fi
done
[[ -n "$clang_bin" ]] || die 'Install a distribution clang, for example clang-19'

# Debian and Ubuntu suffix LLVM tools with a version; accept either naming.
llvm_tool() {
    local name="$1" versioned
    versioned="$(command -v "${name}-19" 2>/dev/null || true)"
    if [[ -n "$versioned" ]]; then printf '%s' "$versioned"; return 0; fi
    command -v "$name" 2>/dev/null
}

strip_bin="$(llvm_tool llvm-strip || true)"
lld_bin="$(llvm_tool ld.lld || true)"
[[ -n "$strip_bin" ]] || die 'Install llvm-strip, provided by the llvm package'
[[ -n "$lld_bin" ]] || die 'Install ld.lld, provided by the lld package'

clang_resource="$("$clang_bin" -print-resource-dir)"
[[ -d "$clang_resource" ]] || die "Cannot locate the clang resource directory for $clang_bin"

builtins_src="$(ls -1 "$NDK_PREBUILT"/lib/clang/*/lib/linux/libclang_rt.builtins-aarch64-android.a 2>/dev/null | head -1 || true)"
unwind_src="$(ls -1 "$NDK_PREBUILT"/lib/clang/*/lib/linux/aarch64/libunwind.a 2>/dev/null | head -1 || true)"
[[ -n "$builtins_src" ]] || die 'Cannot find libclang_rt.builtins-aarch64-android.a in the NDK'
[[ -n "$unwind_src" ]] || die 'Cannot find libunwind.a in the NDK'

api_levels="$(ls -d "$SYSROOT"/usr/lib/aarch64-linux-android/*/ 2>/dev/null | xargs -r -n1 basename | sort -n)"
[[ -n "$api_levels" ]] || die "No aarch64 API levels under $SYSROOT"

target_bin="${ANDROID_HOST_TOOLS:-$HOME/.local/bin}"
changed=0
pending() {
    changed=$((changed + 1))
    log "  missing: $*"
}

log "Host: $host_arch  NDK: $ANDROID_NDK_HOME  clang: $clang_bin"

# rustc's android target links -lunwind, which the NDK keeps beside its clang
# runtimes rather than in the sysroot's per-API library directories.
for api in $api_levels; do
    target="$SYSROOT/usr/lib/aarch64-linux-android/$api/libunwind.a"
    [[ -f "$target" ]] && continue
    if ((CHECK_ONLY)); then pending "libunwind.a for API $api"; else
        cp "$unwind_src" "$target"
        log "  added: libunwind.a for API $api"
    fi
done

# rustc also links -lgcc by default; selecting compiler-rt still needs the
# builtins archive under the triple clang derives from --target.
for api in $api_levels; do
    dir="$clang_resource/lib/aarch64-unknown-linux-android$api"
    [[ -f "$dir/libclang_rt.builtins.a" ]] && continue
    if ((CHECK_ONLY)); then pending "libclang_rt.builtins.a for API $api"; else
        mkdir -p "$dir"
        cp "$builtins_src" "$dir/libclang_rt.builtins.a"
        log "  added: libclang_rt.builtins.a for API $api"
    fi
done

# .cargo/config.toml names the linker after an NDK wrapper. Provide wrappers
# that drive the native clang with the NDK sysroot instead of the x86-64 driver.
# The unsuffixed name matters: aws-lc-sys builds through the `cc` crate, which
# resolves the compiler from the target triple without an API suffix.
for api in $api_levels; do
    for name in "aarch64-linux-android${api}-clang"; do
        path="$target_bin/$name"
        [[ -f "$path" ]] && continue
        if ((CHECK_ONLY)); then pending "linker wrapper $name"; else
            mkdir -p "$target_bin"
            cat > "$path" <<WRAPPER
#!/bin/sh
exec "$clang_bin" --target=aarch64-linux-android${api} --sysroot="$SYSROOT" -rtlib=compiler-rt "\$@"
WRAPPER
            chmod +x "$path"
            log "  added: $name"
        fi
    done
done

for name in aarch64-linux-android-clang aarch64-linux-android-gcc; do
    path="$target_bin/$name"
    [[ -f "$path" ]] && continue
    if ((CHECK_ONLY)); then pending "linker alias $name"; else
        mkdir -p "$target_bin"
        cat > "$path" <<WRAPPER
#!/bin/sh
exec "$clang_bin" --target=aarch64-linux-android26 --sysroot="$SYSROOT" -rtlib=compiler-rt "\$@"
WRAPPER
        chmod +x "$path"
        log "  added: $name"
    fi
done

ar_bin="$(llvm_tool llvm-ar || true)"
if [[ -n "$ar_bin" && ! -f "$target_bin/aarch64-linux-android-ar" ]]; then
    if ((CHECK_ONLY)); then pending 'archiver alias aarch64-linux-android-ar'; else
        mkdir -p "$target_bin"
        printf '#!/bin/sh\nexec "%s" "$@"\n' "$ar_bin" > "$target_bin/aarch64-linux-android-ar"
        chmod +x "$target_bin/aarch64-linux-android-ar"
        log '  added: aarch64-linux-android-ar'
    fi
fi

# Tooling that runs out of the NDK directory must not execute x86-64 binaries;
# stage-native-payload.py uses bin/llvm-strip from there.
for tool in llvm-strip llvm-objcopy llvm-nm llvm-ar llvm-ranlib ld.lld llvm-readelf llvm-objdump; do
    path="$NDK_PREBUILT/bin/$tool"
    [[ -e "$path" ]] || continue
    [[ -f "$path.orig-x86" ]] && continue
    [[ "$(head -c 2 "$path" 2>/dev/null)" == '#!' ]] && continue
    native="$(llvm_tool "$tool" || true)"
    [[ -n "$native" ]] || continue
    if ((CHECK_ONLY)); then pending "NDK bin/$tool replacement"; else
        cp -a "$(readlink -f "$path")" "$path.orig-x86"
        printf '#!/bin/sh\nexec "%s" "$@"\n' "$native" > "$path"
        chmod +x "$path"
        log "  redirected: NDK bin/$tool"
    fi
done

if [[ "$(head -c 4 "$NDK_PREBUILT/bin/clang" 2>/dev/null | od -An -tx1 | tr -d ' \n')" != '23212f62' ]]; then
    if ((CHECK_ONLY)); then pending 'NDK bin/clang replacement'; else
        cp -a "$(readlink -f "$NDK_PREBUILT/bin/clang")" "$NDK_PREBUILT/bin/clang.orig-x86"
        cat > "$NDK_PREBUILT/bin/clang" <<WRAPPER
#!/bin/sh
exec "$clang_bin" --sysroot="$SYSROOT" -rtlib=compiler-rt "\$@"
WRAPPER
        chmod +x "$NDK_PREBUILT/bin/clang"
        log '  redirected: NDK bin/clang'
    fi
fi

if ((CHECK_ONLY)); then
    if ((changed)); then
        log "Incomplete: $changed item(s) missing. Re-run without --check to apply."
        exit 1
    fi
    log 'Host is ready to link the Android targets with native clang.'
    exit 0
fi

log ''
log "Ensure $target_bin is on PATH, then verify:"
log '  cargo build -p rua-cli --target aarch64-linux-android --features mobile-default'
