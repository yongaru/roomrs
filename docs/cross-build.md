# 크로스 빌드 가이드 (Linux · Android)

zig를 C 컴파일러/링커로 써서 Windows에서 리눅스·안드로이드 산출물을 뽑는다.
bundled SQLite(C 소스)도 zig cc가 함께 컴파일한다.

## 요구 도구

| 도구 | 설치 | 확인 |
|---|---|---|
| zig | https://ziglang.org (또는 `winget install zig.zig`) | `zig version` |
| cargo-zigbuild | `cargo install cargo-zigbuild` | `cargo zigbuild --help` |
| cargo-ndk | `cargo install cargo-ndk` | `cargo ndk --version` |
| Android NDK | Android Studio SDK Manager | `ANDROID_HOME` 또는 `ANDROID_NDK_HOME` |

rust 타깃:

```
rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-unknown-linux-musl
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
```

## 실행

```
cargo xtask cross-linux      # linux gnu x64/arm64 (roomrs + FFI .so) + musl x64 (roomrs + CLI 정적)
cargo xtask cross-android    # arm64-v8a / armeabi-v7a / x86_64 .so
cargo xtask cross-all
```

수동 실행 예:

```
cargo zigbuild --target aarch64-unknown-linux-gnu -p roomrs-mobile-ffi-example
cargo ndk -t arm64-v8a build -p roomrs-mobile-ffi-example
```

## 산출물 (target 디렉터리 기준)

```
<target>/x86_64-unknown-linux-gnu/debug/libroomrs_mobile_ffi_example.so
<target>/aarch64-unknown-linux-gnu/debug/libroomrs_mobile_ffi_example.so
<target>/x86_64-unknown-linux-musl/debug/roomrs            # 정적 CLI
<target>/aarch64-linux-android/debug/libroomrs_mobile_ffi_example.so
<target>/armv7-linux-androideabi/debug/libroomrs_mobile_ffi_example.so
<target>/x86_64-linux-android/debug/libroomrs_mobile_ffi_example.so
```

릴리스는 `--release` 추가(xtask는 debug 기본 — 검증 목적).

## 알려진 제약

- **Android는 zig 단독 불가**:
  1. zig 자체가 bionic libc를 제공하지 않는다 —
     `zig cc -target aarch64-linux-android` 실행 시
     `error: unable to provide libc for target '…-android.29'` (제공 목록은 gnu/musl뿐).
     bionic 헤더/crt는 NDK에만 있다 → libsqlite3-sys의 sqlite3.c 컴파일 불가.
  2. 그 이전에 cargo-zigbuild 0.22.1의 Windows `.bat` 래퍼를 cc-rs가 잘못 파싱해
     (`failed to find tool "…\8571.exe"`) build script가 먼저 죽는다.
  결론: Android는 cargo-ndk와 NDK 경로를 사용한다.
  (NDK sysroot를 zig에 `--sysroot`로 넘기는 우회는 가능하지만, 그 시점에 NDK가 있으므로 무의미.)
- **iOS/macOS** — Apple SDK와 macOS 호스트(Xcode)가 필요하다. CI macOS 러너에서
  `aarch64-apple-ios` 타깃을 빌드한다.
- 크로스 산출물의 실기기/에뮬레이터 실행 테스트는 별도다.
