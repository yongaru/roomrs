#[cfg(feature = "sqlcipher-system")]
use std::env;
#[cfg(feature = "sqlcipher-system")]
use std::path::PathBuf;

/// Windows MSVC system SQLCipher의 정적 OpenSSL 의존성을 vcpkg에서 전달한다.
#[cfg(feature = "sqlcipher-system")]
fn main() {
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    println!("cargo:rerun-if-env-changed=VCPKGRS_TRIPLET");
    println!("cargo:rerun-if-env-changed=VCPKGRS_DYNAMIC");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }

    let Some(root) = env::var_os("VCPKG_ROOT") else {
        eprintln!("system SQLCipher에는 VCPKG_ROOT가 필요합니다.");
        std::process::exit(1);
    };
    let Some(triplet) = env::var_os("VCPKGRS_TRIPLET") else {
        eprintln!("system SQLCipher에는 VCPKGRS_TRIPLET이 필요합니다.");
        std::process::exit(1);
    };
    let library_directory = PathBuf::from(root)
        .join("installed")
        .join(triplet)
        .join("lib");
    for library in ["libssl.lib", "libcrypto.lib"] {
        if !library_directory.join(library).is_file() {
            eprintln!(
                "system SQLCipher용 vcpkg OpenSSL library가 없습니다: {}",
                library_directory.join(library).display()
            );
            std::process::exit(1);
        }
    }

    println!(
        "cargo:rustc-link-search=native={}",
        library_directory.display()
    );
    println!("cargo:rustc-link-lib=static=libssl");
    println!("cargo:rustc-link-lib=static=libcrypto");
    for library in ["crypt32", "ws2_32", "advapi32", "user32"] {
        println!("cargo:rustc-link-lib=dylib={library}");
    }
}

/// system SQLCipher가 아니면 추가 native link metadata를 만들지 않는다.
#[cfg(not(feature = "sqlcipher-system"))]
fn main() {}
