@echo off
:: SPDX-License-Identifier: AGPL-3.0-only
:: Copyright (c) 2026 sol pbc
::
:: First native Windows journal gate. It proves the source-bound MSVC transport
:: and the portable journal/config substrate only. It builds journal-io,
:: journal, and journal-config; runs journal-io library and lock-component
:: tests, journal-config unit tests, and journal library tests. The ordinary-owner
:: inventory control is mandatory; Cloud Files registration remains separately opt-in.
:: ReFS archive traversal runs when its fixture root is configured. Detailed atomic
:: publication runs only when its ReFS receipt is required. Locking beyond the named
:: component, Callosum, packaging, install, signing, and smoke remain later gates.
setlocal enableextensions
cd /d "%~dp0.." || exit /b 1

set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

if not defined EXPECTED_JOURNAL_COMMIT ( echo ERROR: EXPECTED_JOURNAL_COMMIT is required; rerun through win-host-ci & exit /b 1 )
if not defined EXPECTED_JOURNAL_CARGO_LOCK_SHA256 ( echo ERROR: EXPECTED_JOURNAL_CARGO_LOCK_SHA256 is required; rerun through win-host-ci & exit /b 1 )
if not defined SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT ( echo ERROR: SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT is required; rerun through win-host-ci & exit /b 1 )
powershell -NoProfile -Command "if ($env:EXPECTED_JOURNAL_COMMIT -notmatch '^[0-9a-f]{40}$' -or $env:EXPECTED_JOURNAL_CARGO_LOCK_SHA256 -notmatch '^[0-9a-f]{64}$') { exit 1 }" || ( echo ERROR: source-binding values must be lowercase full commit and SHA-256 hex; rerun through win-host-ci & exit /b 1 )

call :verify_source_binding || exit /b 1

if not defined JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST set "JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST=0"
powershell -NoProfile -Command "if ($env:JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST -notmatch '^[01]$') { exit 1 }" || ( echo ERROR: JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST must be 0 or 1; rerun through win-host-ci & exit /b 1 )
if not defined JOURNAL_WIN_CI_REQUIRE_REFS_PUBLICATION set "JOURNAL_WIN_CI_REQUIRE_REFS_PUBLICATION=0"
powershell -NoProfile -Command "if ($env:JOURNAL_WIN_CI_REQUIRE_REFS_PUBLICATION -notmatch '^[01]$') { exit 1 }" || ( echo ERROR: JOURNAL_WIN_CI_REQUIRE_REFS_PUBLICATION must be 0 or 1; rerun through win-host-ci & exit /b 1 )
if not defined SOLSTONE_JOURNAL_WIN_REFS_ROOT set "SOLSTONE_JOURNAL_WIN_REFS_ROOT="
if "%JOURNAL_WIN_CI_REQUIRE_REFS_PUBLICATION%"=="1" if not defined SOLSTONE_JOURNAL_WIN_REFS_ROOT ( echo ERROR: JOURNAL_WIN_CI_REQUIRE_REFS_PUBLICATION=1 requires SOLSTONE_JOURNAL_WIN_REFS_ROOT; rerun through win-host-ci & exit /b 1 )

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" ( echo ERROR: vswhere not found at "%VSWHERE%" & exit /b 1 )
set "VSINSTALL="
for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSINSTALL=%%i"
if not defined VSINSTALL ( echo ERROR: VS Build Tools with VC.Tools.x86.x64 not found & exit /b 1 )
call "%VSINSTALL%\VC\Auxiliary\Build\vcvarsall.bat" x64 >nul || ( echo ERROR: vcvarsall failed & exit /b 1 )

echo === cargo build --locked (portable journal substrate) ===
cargo build --manifest-path core\Cargo.toml --locked -p solstone-core-journal -p solstone-core-journal-config -p solstone-core-journal-io -p solstone-core-win-owner-rail || exit /b 1
echo === cargo test --locked (portable journal config substrate) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-config --lib || exit /b 1
set "JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=skipped"
set "JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=failed"
set "JOURNAL_WIN_CI_REFS_PUBLICATION=unrun/skipped"
set "JOURNAL_WIN_CI_REFS_PUBLICATION_FILESYSTEM=not-asserted"
set "JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE=unrun/skipped"
set "JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY=not-asserted"
set "JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE=unrun/skipped"
set "JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY=not-asserted"
set "JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE=unrun/skipped"
set "JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY=unsupported"
set "JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE=unrun/skipped"
set "JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY=not-asserted"
if "%JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST%"=="1" (
  echo === cargo test --locked journal-io Cloud Files sync-root registration ===
  cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test windows_cloud_sync_root_registration --features test-hooks || exit /b 1
  set "JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=passed"
) else (
  echo === Cloud Files sync-root registration test not run; set JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST=1 to include it ===
)
echo === cargo test --locked journal-io ordinary-owner inventory control ===
set "JOURNAL_WIN_CI_OWNER_RAIL=core\target\debug\solstone-core-win-owner-rail.exe"
set "JOURNAL_WIN_CI_OWNER_LEASE=C:\ProgramData\solstone\journal-win-owner-rail\ordinary-owner.lease.json"
"%JOURNAL_WIN_CI_OWNER_RAIL%" recover-held --lease "%JOURNAL_WIN_CI_OWNER_LEASE%" || goto :ordinary_owner_failed
"%JOURNAL_WIN_CI_OWNER_RAIL%" prepare --lease "%JOURNAL_WIN_CI_OWNER_LEASE%" --worktree "%CD%" --worker "%CD%\%JOURNAL_WIN_CI_OWNER_RAIL%" --expected-commit "%EXPECTED_JOURNAL_COMMIT%" --expected-lock "%EXPECTED_JOURNAL_CARGO_LOCK_SHA256%" --owner-account "%SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT%" --refs-root-env SOLSTONE_JOURNAL_WIN_REFS_ROOT || goto :ordinary_owner_failed
"%JOURNAL_WIN_CI_OWNER_RAIL%" launch --lease "%JOURNAL_WIN_CI_OWNER_LEASE%" || goto :ordinary_owner_cleanup_failed
set "JOURNAL_WIN_CI_ORDINARY_OWNER_LOG=core\target\journal-win-ci-ordinary-owner-%RANDOM%%RANDOM%.log"
"%JOURNAL_WIN_CI_OWNER_RAIL%" await --lease "%JOURNAL_WIN_CI_OWNER_LEASE%" > "%JOURNAL_WIN_CI_ORDINARY_OWNER_LOG%" 2>&1
set "JOURNAL_WIN_CI_ORDINARY_OWNER_STATUS=%ERRORLEVEL%"
type "%JOURNAL_WIN_CI_ORDINARY_OWNER_LOG%"
powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_ORDINARY_OWNER_LOG); if ([regex]::Matches($text, '(?m)^JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed\r?$').Count -eq 1) { exit 0 }; exit 1"
set "JOURNAL_WIN_CI_ORDINARY_OWNER_MARKER_STATUS=%ERRORLEVEL%"
if defined SOLSTONE_JOURNAL_WIN_REFS_ROOT powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_ORDINARY_OWNER_LOG); if ([regex]::Matches($text, '(?m)^JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=passed\r?$').Count -eq 1) { exit 0 }; exit 1"
if defined SOLSTONE_JOURNAL_WIN_REFS_ROOT set "JOURNAL_WIN_CI_ORDINARY_OWNER_REFS_STATUS=%ERRORLEVEL%"
if not "%JOURNAL_WIN_CI_ORDINARY_OWNER_STATUS%"=="0" (
  rem `cleanup` itself verifies TerminalVerified, so an unbound, timed-out, or
  rem scheduler-uncertain outcome remains held and this cannot delete it.
  "%JOURNAL_WIN_CI_OWNER_RAIL%" cleanup --lease "%JOURNAL_WIN_CI_OWNER_LEASE%" || goto :ordinary_owner_failed
  goto :ordinary_owner_failed
)
if not "%JOURNAL_WIN_CI_ORDINARY_OWNER_MARKER_STATUS%"=="0" goto :ordinary_owner_failed
if defined SOLSTONE_JOURNAL_WIN_REFS_ROOT if not "%JOURNAL_WIN_CI_ORDINARY_OWNER_REFS_STATUS%"=="0" goto :ordinary_owner_failed
"%JOURNAL_WIN_CI_OWNER_RAIL%" cleanup --lease "%JOURNAL_WIN_CI_OWNER_LEASE%" || goto :ordinary_owner_failed
del /q "%JOURNAL_WIN_CI_ORDINARY_OWNER_LOG%" >nul 2>&1
set "JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=passed"
if "%JOURNAL_WIN_CI_REQUIRE_REFS_PUBLICATION%"=="1" call :run_refs_publication || exit /b 1
if defined SOLSTONE_JOURNAL_WIN_REFS_ROOT call :run_refs_matrix || exit /b 1
echo === cargo test --locked (journal-io library) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --lib || exit /b 1
echo === cargo test --locked (journal-io lock component) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test journal_io_lock_component --features test-hooks || exit /b 1
echo === cargo test --locked (journal-io detailed atomic publication) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test windows_atomic_detailed --features test-hooks || exit /b 1
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
echo JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=%JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE%
if "%JOURNAL_WIN_CI_REQUIRE_REFS_PUBLICATION%"=="1" echo JOURNAL_WIN_CI_REFS_PUBLICATION=%JOURNAL_WIN_CI_REFS_PUBLICATION%
if defined SOLSTONE_JOURNAL_WIN_REFS_ROOT (
  echo JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE=%JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE%
  echo JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY=%JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY%
  echo JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE=%JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE%
  echo JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY=%JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY%
  echo JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE=%JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE%
  echo JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY=%JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY%
  echo JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE=%JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE%
  echo JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY=%JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY%
)
echo === JOURNAL_WIN_CI_OK: native Windows MSVC build passed for solstone-core-journal-io solstone-core-journal and solstone-core-journal-config; journal-io library lock-component and detailed-publication tests and journal library tests including config_strip_matches_python_control_whitespace and ensure_journal_dir_reports_non_directory_parent passed; ordinary-owner inventory evidence %JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE%; Cloud Files sync-root registration evidence %JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE%; ReFS publication evidence %JOURNAL_WIN_CI_REFS_PUBLICATION% filesystem %JOURNAL_WIN_CI_REFS_PUBLICATION_FILESYSTEM%; ReFS enumeration evidence %JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE% capability %JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY%; ReFS revalidation evidence %JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE% capability %JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY%; ReFS claimed-removal evidence %JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE% capability %JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY%; ReFS archive traversal evidence %JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE% capability %JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY%; publication locking beyond the detailed publication path Callosum packaging install signing and smoke not run ===
exit /b 0

:ordinary_owner_failed
set "JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=failed"
echo ERROR: ordinary-owner inventory control did not both exit successfully and emit JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed
exit /b 1

:ordinary_owner_cleanup_failed
goto :ordinary_owner_failed

:run_refs_publication
echo === cargo test --locked journal-io ReFS detailed publication receipt ===
set "JOURNAL_WIN_CI_REFS_PUBLICATION_LOG=core\target\journal-win-ci-refs-publication-%RANDOM%%RANDOM%.log"
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test windows_atomic_detailed --features test-hooks -- --ignored --exact refs_publication_receipt --nocapture > "%JOURNAL_WIN_CI_REFS_PUBLICATION_LOG%" 2>&1
set "JOURNAL_WIN_CI_REFS_PUBLICATION_STATUS=%ERRORLEVEL%"
type "%JOURNAL_WIN_CI_REFS_PUBLICATION_LOG%"
if not "%JOURNAL_WIN_CI_REFS_PUBLICATION_STATUS%"=="0" exit /b 1
powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_REFS_PUBLICATION_LOG); if ([regex]::Matches($text, '(?m)^JOURNAL_WIN_CI_REFS_PUBLICATION_FILESYSTEM=ReFS\r?$').Count -eq 1) { exit 0 }; exit 1"
if not "%ERRORLEVEL%"=="0" ( echo ERROR: ReFS detailed publication receipt did not emit one runtime-observed filesystem marker & exit /b 1 )
del /q "%JOURNAL_WIN_CI_REFS_PUBLICATION_LOG%" >nul 2>&1
set "JOURNAL_WIN_CI_REFS_PUBLICATION=executed/pass"
set "JOURNAL_WIN_CI_REFS_PUBLICATION_FILESYSTEM=ReFS"
exit /b 0

:run_refs_matrix
set "JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE=executed/pass"
set "JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY=available"
set "JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE=executed/pass"
set "JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY=available"
set "JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE=executed/refusal"
set "JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY=unsupported"
set "JOURNAL_WIN_CI_REFS_ARCHIVE_TEMP=%TEMP%"
set "JOURNAL_WIN_CI_REFS_ARCHIVE_TMP=%TMP%"
set "TEMP=%SOLSTONE_JOURNAL_WIN_REFS_ROOT%"
set "TMP=%SOLSTONE_JOURNAL_WIN_REFS_ROOT%"
echo === cargo test --locked journal-archive ReFS source traversal ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-archive --lib source_freezes_portable_members_and_checked_bytes -- --nocapture
set "JOURNAL_WIN_CI_REFS_ARCHIVE_STATUS=%ERRORLEVEL%"
set "TEMP=%JOURNAL_WIN_CI_REFS_ARCHIVE_TEMP%"
set "TMP=%JOURNAL_WIN_CI_REFS_ARCHIVE_TMP%"
if not "%JOURNAL_WIN_CI_REFS_ARCHIVE_STATUS%"=="0" exit /b 1
set "JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE=executed/pass"
set "JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY=available"
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
