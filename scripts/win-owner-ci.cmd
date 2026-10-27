@echo off
:: SPDX-License-Identifier: AGPL-3.0-only
:: Copyright (c) 2026 sol pbc
:: The limited scheduled-task child needs its own MSVC environment.
setlocal enableextensions
cd /d "%~dp0.." || exit /b 1
if not defined CARGO_TARGET_DIR ( echo ERROR: owner rail target directory is required & exit /b 1 )
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
set "CARGO_INCREMENTAL=0"
set "CARGO_PROFILE_DEV_DEBUG=0"
set "CARGO_BUILD_TARGET=x86_64-pc-windows-msvc"
set "CARGO_ENCODED_RUSTFLAGS=-Ctarget-feature=+crt-static"
set "RUSTFLAGS="
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" ( echo ERROR: vswhere not found & exit /b 1 )
set "VSINSTALL="
for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSINSTALL=%%i"
if not defined VSINSTALL ( echo ERROR: VS Build Tools with VC.Tools.x86.x64 not found & exit /b 1 )
call "%VSINSTALL%\VC\Auxiliary\Build\vcvarsall.bat" x64 >nul || ( echo ERROR: vcvarsall failed & exit /b 1 )
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test windows_ordinary_owner_inventory --features test-hooks -- --nocapture
exit /b %ERRORLEVEL%
