@echo off
:: SPDX-License-Identifier: AGPL-3.0-only
:: Copyright (c) 2026 sol pbc
::
:: First native Windows journal gate. It proves the source-bound MSVC transport
:: and the portable journal/config substrate only. It builds journal-io,
:: journal, and journal-config; runs journal-io library and lock-component
:: tests, journal-config unit tests, and journal library tests. The side-effecting
:: Cloud Files registration test is separately opt-in. Archive, publication,
:: locking beyond the named component, Callosum, packaging, install, signing,
:: smoke, and full NTFS/ReFS evidence remain later gates.
setlocal enableextensions
cd /d "%~dp0.." || exit /b 1

set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

if not defined EXPECTED_JOURNAL_COMMIT ( echo ERROR: EXPECTED_JOURNAL_COMMIT is required; rerun through win-host-ci & exit /b 1 )
if not defined EXPECTED_JOURNAL_CARGO_LOCK_SHA256 ( echo ERROR: EXPECTED_JOURNAL_CARGO_LOCK_SHA256 is required; rerun through win-host-ci & exit /b 1 )
powershell -NoProfile -Command "if ($env:EXPECTED_JOURNAL_COMMIT -notmatch '^[0-9a-f]{40}$' -or $env:EXPECTED_JOURNAL_CARGO_LOCK_SHA256 -notmatch '^[0-9a-f]{64}$') { exit 1 }" || ( echo ERROR: source-binding values must be lowercase full commit and SHA-256 hex; rerun through win-host-ci & exit /b 1 )

call :verify_source_binding || exit /b 1

if not defined JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST set "JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST=0"
powershell -NoProfile -Command "if ($env:JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST -notmatch '^[01]$') { exit 1 }" || ( echo ERROR: JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST must be 0 or 1; rerun through win-host-ci & exit /b 1 )
if not defined SOLSTONE_JOURNAL_WIN_REFS_ROOT set "SOLSTONE_JOURNAL_WIN_REFS_ROOT="

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" ( echo ERROR: vswhere not found at "%VSWHERE%" & exit /b 1 )
set "VSINSTALL="
for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSINSTALL=%%i"
if not defined VSINSTALL ( echo ERROR: VS Build Tools with VC.Tools.x86.x64 not found & exit /b 1 )
call "%VSINSTALL%\VC\Auxiliary\Build\vcvarsall.bat" x64 >nul || ( echo ERROR: vcvarsall failed & exit /b 1 )

echo === cargo build --locked (portable journal substrate) ===
cargo build --manifest-path core\Cargo.toml --locked -p solstone-core-journal -p solstone-core-journal-config -p solstone-core-journal-io || exit /b 1
echo === cargo test --locked (portable journal config substrate) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-config --lib || exit /b 1
set "JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=skipped"
set "JOURNAL_WIN_CI_REFS_WITNESS_EVIDENCE=skipped"
set "JOURNAL_WIN_CI_REFS_WITNESS_ROOT="
set "JOURNAL_WIN_CI_REFS_WITNESS_FILESYSTEM=unavailable"
if "%JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST%"=="1" (
  echo === cargo test --locked journal-io Cloud Files sync-root registration ===
  cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test windows_cloud_sync_root_registration --features test-hooks || exit /b 1
  set "JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=passed"
) else (
  echo === Cloud Files sync-root registration test not run; set JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST=1 to include it ===
)
if defined SOLSTONE_JOURNAL_WIN_REFS_ROOT (
  for /f "tokens=1,* delims=|" %%A in ('powershell -NoProfile -Command "$root = $env:SOLSTONE_JOURNAL_WIN_REFS_ROOT; try { $item = Get-Item -LiteralPath $root -Force -ErrorAction Stop; if (-not $item.PSIsContainer) { exit 2 }; $volume = Get-Volume -FilePath $item.FullName -ErrorAction Stop; [Console]::Write($item.FullName + '|' + $volume.FileSystem) } catch { exit 1 }"') do (
    set "JOURNAL_WIN_CI_REFS_WITNESS_ROOT=%%A"
    set "JOURNAL_WIN_CI_REFS_WITNESS_FILESYSTEM=%%B"
  )
  if not defined JOURNAL_WIN_CI_REFS_WITNESS_ROOT (
    set "JOURNAL_WIN_CI_REFS_WITNESS_EVIDENCE=unsupported"
  ) else if /I not "%JOURNAL_WIN_CI_REFS_WITNESS_FILESYSTEM%"=="ReFS" (
    set "JOURNAL_WIN_CI_REFS_WITNESS_EVIDENCE=unsupported"
  ) else (
    echo === cargo test --locked journal-io ReFS witness controls ===
    cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --lib real_ntfs_and_refs_controls_skip_without_environment || exit /b 1
    cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --lib real_ntfs_and_refs_witness_mutation_controls_skip_without_environment || exit /b 1
    cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --lib real_ntfs_and_refs_witness_overflow_controls_skip_without_environment || exit /b 1
    set "JOURNAL_WIN_CI_REFS_WITNESS_EVIDENCE=passed"
  )
) else (
  echo === ReFS witness controls not run; set SOLSTONE_JOURNAL_WIN_REFS_ROOT to a ReFS fixture directory ===
)
echo === cargo test --locked (journal-io library) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --lib || exit /b 1
echo === cargo test --locked (journal-io lock component) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test journal_io_lock_component --features test-hooks || exit /b 1
echo === checking required journal portability tests ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal --lib -- --list | findstr /c:"tests::config_strip_matches_python_control_whitespace: test" >nul || ( echo ERROR: required journal test config_strip_matches_python_control_whitespace is missing & exit /b 1 )
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal --lib -- --list | findstr /c:"tests::ensure_journal_dir_reports_non_directory_parent: test" >nul || ( echo ERROR: required journal test ensure_journal_dir_reports_non_directory_parent is missing & exit /b 1 )
call :require_journal_test tests::config_strip_matches_python_control_whitespace || exit /b 1
call :require_journal_test tests::ensure_journal_dir_reports_non_directory_parent || exit /b 1
echo === cargo test --locked (journal library) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal --lib || exit /b 1

:: Detect another operator replacing the persistent checkout while Cargo ran.
:: The driver-side lock normally serializes this rail; this second check keeps
:: an out-of-band checkout from earning a source-bound success marker.
call :verify_source_binding || exit /b 1

echo JOURNAL_WIN_CI_HEAD=%JOURNAL_WIN_CI_HEAD%
echo JOURNAL_WIN_CI_CARGO_LOCK_SHA256=%JOURNAL_WIN_CI_CARGO_LOCK_SHA256%
echo JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=%JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE%
echo JOURNAL_WIN_CI_REFS_WITNESS_EVIDENCE=%JOURNAL_WIN_CI_REFS_WITNESS_EVIDENCE%
echo JOURNAL_WIN_CI_REFS_WITNESS_ROOT=%JOURNAL_WIN_CI_REFS_WITNESS_ROOT%
echo JOURNAL_WIN_CI_REFS_WITNESS_FILESYSTEM=%JOURNAL_WIN_CI_REFS_WITNESS_FILESYSTEM%
echo === JOURNAL_WIN_CI_OK: native Windows MSVC build passed for solstone-core-journal-io solstone-core-journal and solstone-core-journal-config; journal-io library and lock-component tests and journal library tests including config_strip_matches_python_control_whitespace and ensure_journal_dir_reports_non_directory_parent passed; Cloud Files sync-root registration evidence %JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE%; ReFS witness evidence %JOURNAL_WIN_CI_REFS_WITNESS_EVIDENCE%; archive publication locking beyond the named lock component Callosum packaging install signing smoke and full NTFS native evidence not run ===
exit /b 0

:require_journal_test
set "JOURNAL_WIN_CI_TEST=%~1"
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal --lib -- --exact "%JOURNAL_WIN_CI_TEST%" 2>&1 | findstr /c:"test result: ok. 1 passed;" >nul || ( echo ERROR: required journal test %JOURNAL_WIN_CI_TEST% is missing ignored or failed & exit /b 1 )
exit /b 0

:verify_source_binding
git rev-parse HEAD >nul 2>&1 || ( echo ERROR: git rev-parse HEAD failed; restore the transferred checkout and retry & exit /b 1 )
set "JOURNAL_WIN_CI_HEAD="
for /f "usebackq tokens=*" %%i in (`git rev-parse HEAD`) do set "JOURNAL_WIN_CI_HEAD=%%i"
if not defined JOURNAL_WIN_CI_HEAD ( echo ERROR: git rev-parse HEAD returned no commit; restore the transferred checkout and retry & exit /b 1 )
if not "%JOURNAL_WIN_CI_HEAD%"=="%EXPECTED_JOURNAL_COMMIT%" ( echo ERROR: transferred HEAD does not match EXPECTED_JOURNAL_COMMIT; restore the transferred bundle and retry & exit /b 1 )

git status --porcelain=v1 --untracked-files=all --ignore-submodules=none >nul 2>&1 || ( echo ERROR: git status failed; restore the transferred checkout and retry & exit /b 1 )
set "JOURNAL_WIN_CI_DIRTY="
for /f "usebackq delims=" %%i in (`git status --porcelain=v1 --untracked-files=all --ignore-submodules=none`) do set "JOURNAL_WIN_CI_DIRTY=1"
if defined JOURNAL_WIN_CI_DIRTY ( echo ERROR: transferred checkout is dirty; restore the exact clean bundle and retry & exit /b 1 )

set "JOURNAL_WIN_CI_CARGO_LOCK_SHA256="
for /f "usebackq tokens=*" %%i in (`powershell -NoProfile -Command "(Get-FileHash -LiteralPath 'core/Cargo.lock' -Algorithm SHA256).Hash.ToLowerInvariant()"`) do set "JOURNAL_WIN_CI_CARGO_LOCK_SHA256=%%i"
if not defined JOURNAL_WIN_CI_CARGO_LOCK_SHA256 ( echo ERROR: core/Cargo.lock SHA-256 could not be computed; restore the tracked lockfile and retry & exit /b 1 )
if not "%JOURNAL_WIN_CI_CARGO_LOCK_SHA256%"=="%EXPECTED_JOURNAL_CARGO_LOCK_SHA256%" ( echo ERROR: core/Cargo.lock SHA-256 does not match the transferred binding; restore the exact lockfile and retry & exit /b 1 )
exit /b 0
