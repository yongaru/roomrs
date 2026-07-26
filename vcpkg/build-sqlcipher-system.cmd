@echo off
setlocal EnableExtensions EnableDelayedExpansion
chcp 65001 >nul

if /i "%~1"=="--help" goto :help
if /i "%~1"=="-h" goto :help

set "TRIPLET=%~1"
if not defined TRIPLET set "TRIPLET=x64-windows-static-release"
set "SQLCIPHER_FEATURES=fts5,geopoly,json1,tool"
set "OVERLAY_ROOT=%~dp0ports"

if not exist "%OVERLAY_ROOT%\sqlcipher\vcpkg.json" (
    echo [sqlcipher-system] overlay port를 찾지 못했습니다: %OVERLAY_ROOT%\sqlcipher
    exit /b 2
)

if defined VCPKG_ROOT (
    set "VCPKG_EXE=%VCPKG_ROOT%\vcpkg.exe"
) else (
    for %%I in (vcpkg.exe) do set "VCPKG_EXE=%%~$PATH:I"
    if defined VCPKG_EXE for %%I in ("!VCPKG_EXE!") do set "VCPKG_ROOT=%%~dpI"
)

if not defined VCPKG_EXE (
    echo [sqlcipher-system] vcpkg.exe를 찾지 못했습니다. VCPKG_ROOT를 설정하거나 vcpkg를 PATH에 추가하세요.
    exit /b 2
)
if not exist "!VCPKG_EXE!" (
    echo [sqlcipher-system] vcpkg.exe가 없습니다: !VCPKG_EXE!
    exit /b 2
)

echo [sqlcipher-system] SQLCipher를 설치합니다.
echo [sqlcipher-system] triplet: %TRIPLET%
echo [sqlcipher-system] overlay: %OVERLAY_ROOT%\sqlcipher
echo [sqlcipher-system] features: %SQLCIPHER_FEATURES%,rtree,unlock_notify,column_metadata,preupdate_hook

pushd "%~dp0" >nul
call "!VCPKG_EXE!" install "sqlcipher[%SQLCIPHER_FEATURES%]:%TRIPLET%" "--overlay-ports=%OVERLAY_ROOT%" --recurse
set "VCPKG_EXIT=!ERRORLEVEL!"
popd >nul
if not "!VCPKG_EXIT!"=="0" (
    echo [sqlcipher-system] vcpkg SQLCipher 설치에 실패했습니다.
    exit /b 3
)

set "SQLCIPHER_EXE=!VCPKG_ROOT!\installed\%TRIPLET%\tools\sqlcipher\sqlcipher.exe"
if not exist "!SQLCIPHER_EXE!" (
    echo [sqlcipher-system] SQLCipher 검증 도구가 없습니다: !SQLCIPHER_EXE!
    exit /b 4
)

set "CHECK_FILE=%TEMP%\sqlcipher-system-check-!RANDOM!-!RANDOM!.txt"
set "PREUPDATE_HOOK="
"!SQLCIPHER_EXE!" -batch ":memory:" "SELECT sqlite_compileoption_used('ENABLE_PREUPDATE_HOOK');" > "!CHECK_FILE!"
if errorlevel 1 (
    del /q "!CHECK_FILE!" >nul 2>&1
    echo [sqlcipher-system] SQLCipher compile option 확인에 실패했습니다.
    exit /b 5
)
set /p "PREUPDATE_HOOK=" < "!CHECK_FILE!"
del /q "!CHECK_FILE!" >nul 2>&1
if not "!PREUPDATE_HOOK!"=="1" (
    echo [sqlcipher-system] SQLCipher에 SQLITE_ENABLE_PREUPDATE_HOOK이 없습니다.
    exit /b 6
)

echo [sqlcipher-system] 설치 완료: !VCPKG_ROOT!\installed\%TRIPLET%
echo [sqlcipher-system] Cargo 환경: VCPKG_ROOT=!VCPKG_ROOT! VCPKGRS_TRIPLET=%TRIPLET% SQLCIPHER_STATIC=1 RUSTFLAGS=-C target-feature=+crt-static
exit /b 0

:help
echo 사용법: vcpkg\build-sqlcipher-system.cmd [triplet]
echo 기본 triplet: x64-windows-static-release
echo 전제: VCPKG_ROOT 설정 또는 vcpkg.exe PATH 등록
exit /b 0
