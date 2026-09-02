@echo off
:: SPDX-License-Identifier: AGPL-3.0-only
:: Copyright (c) 2026 sol pbc
::
:: First native Windows journal gate. It proves the source-bound MSVC transport
:: and the portable journal/config substrate only. It builds journal-io,
:: journal, and journal-config; runs journal-io library and lock-component
:: tests, journal-config unit tests, and journal library tests. The ordinary-owner
:: inventory control is mandatory; Cloud Files registration remains separately opt-in.
:: The mandatory native receipt children prove NTFS and ReFS publication and
:: stale-heartbeat cleanup. Their source-originated output is captured and
:: validated below; the runner never synthesizes receipt markers.
:: Keep this runner CRLF-materialized: cmd.exe label calls fail on LF-only rewrites.
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
if not defined SOLSTONE_JOURNAL_WIN_REFS_ROOT ( echo ERROR: SOLSTONE_JOURNAL_WIN_REFS_ROOT is required for mandatory ReFS receipts; rerun through win-host-ci & exit /b 1 )
powershell -NoProfile -Command "if ($env:SOLSTONE_JOURNAL_WIN_REFS_ROOT -notmatch '^[A-Za-z]:[\\/][A-Za-z0-9_. ()\\/:=-]*$') { exit 1 }" || ( echo ERROR: SOLSTONE_JOURNAL_WIN_REFS_ROOT must be a safe absolute Windows path; rerun through win-host-ci & exit /b 1 )

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" ( echo ERROR: vswhere not found at "%VSWHERE%" & exit /b 1 )
set "VSINSTALL="
for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSINSTALL=%%i"
if not defined VSINSTALL ( echo ERROR: VS Build Tools with VC.Tools.x86.x64 not found & exit /b 1 )
call "%VSINSTALL%\VC\Auxiliary\Build\vcvarsall.bat" x64 >nul || ( echo ERROR: vcvarsall failed & exit /b 1 )

echo === cargo build --locked (portable journal substrate) ===
cargo build --manifest-path core\Cargo.toml --locked -p solstone-core-journal -p solstone-core-journal-config -p solstone-core-journal-io -p solstone-core-system -p solstone-core-win-owner-rail || exit /b 1
echo === cargo test --locked (portable journal config substrate) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-config --lib || exit /b 1
set "JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=skipped"
set "JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=failed"
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
powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_ORDINARY_OWNER_LOG); if ([regex]::Matches($text, '(?m)^JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=passed\r?$').Count -eq 1) { exit 0 }; exit 1"
set "JOURNAL_WIN_CI_ORDINARY_OWNER_REFS_STATUS=%ERRORLEVEL%"
if not "%JOURNAL_WIN_CI_ORDINARY_OWNER_STATUS%"=="0" (
  rem `cleanup` itself verifies TerminalVerified, so an unbound, timed-out, or
  rem scheduler-uncertain outcome remains held and this cannot delete it.
  "%JOURNAL_WIN_CI_OWNER_RAIL%" cleanup --lease "%JOURNAL_WIN_CI_OWNER_LEASE%" || goto :ordinary_owner_failed
  goto :ordinary_owner_failed
)
if not "%JOURNAL_WIN_CI_ORDINARY_OWNER_MARKER_STATUS%"=="0" goto :ordinary_owner_failed
if not "%JOURNAL_WIN_CI_ORDINARY_OWNER_REFS_STATUS%"=="0" goto :ordinary_owner_failed
"%JOURNAL_WIN_CI_OWNER_RAIL%" cleanup --lease "%JOURNAL_WIN_CI_OWNER_LEASE%" || goto :ordinary_owner_failed
del /q "%JOURNAL_WIN_CI_ORDINARY_OWNER_LOG%" >nul 2>&1
set "JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=passed"
call :run_platform_receipt "Windows launch environment preparation" "solstone-core-system" "windows_lifecycle_receipt" "windows_launch_environment_preparation_receipt" "JOURNAL_WIN_CI_LAUNCH_ENVIRONMENT_PREPARATION" || exit /b 1
call :run_platform_receipt "Windows launch path preparation" "solstone-core-system" "windows_lifecycle_receipt" "windows_launch_path_preparation_receipt" "JOURNAL_WIN_CI_LAUNCH_PATH_PREPARATION" || exit /b 1
call :run_platform_receipt "Windows JOB_LIST without handle inheritance" "solstone-core-system" "windows_lifecycle_receipt" "windows_job_list_no_handle_inheritance_receipt" "JOURNAL_WIN_CI_JOB_LIST_NO_HANDLE_INHERITANCE" || exit /b 1
call :run_platform_receipt "Windows Job process owner" "solstone-core-system" "windows_lifecycle_receipt" "windows_job_process_owner_receipt" "JOURNAL_WIN_CI_JOB_PROCESS_OWNER" || exit /b 1
call :run_platform_receipt "Windows Job last-handle negative" "solstone-core-system" "windows_lifecycle_receipt" "windows_job_last_handle_negative_receipt" "JOURNAL_WIN_CI_JOB_LAST_HANDLE_NEGATIVE" || exit /b 1
call :run_platform_receipt "Windows managed-process facade" "solstone-core-system" "windows_lifecycle_receipt" "windows_managed_process_facade_receipt" "JOURNAL_WIN_CI_MANAGED_PROCESS_FACADE" || exit /b 1
call :run_receipt "NTFS publication" "solstone-core-journal-io" "windows_atomic_detailed" "ntfs_publication_receipt" "JOURNAL_WIN_CI_NTFS_PUBLICATION" "NTFS" || exit /b 1
call :run_receipt "ReFS publication" "solstone-core-journal-io" "windows_atomic_detailed" "refs_publication_receipt" "JOURNAL_WIN_CI_REFS_PUBLICATION" "ReFS" || exit /b 1
call :run_receipt "NTFS Cortex-use recovery" "solstone-core-journal-io" "windows_atomic_detailed" "ntfs_cortex_use_receipt" "JOURNAL_WIN_CI_CORTEX_USE_NTFS" "NTFS" || exit /b 1
call :run_receipt "ReFS Cortex-use recovery" "solstone-core-journal-io" "windows_atomic_detailed" "refs_cortex_use_receipt" "JOURNAL_WIN_CI_CORTEX_USE_REFS" "ReFS" || exit /b 1
call :run_receipt "NTFS managed-log reference" "solstone-core-journal-io" "windows_atomic_detailed" "ntfs_managed_log_reference_receipt" "JOURNAL_WIN_CI_NTFS_MANAGED_LOG_REFERENCE" "NTFS" || exit /b 1
call :run_receipt "ReFS managed-log reference" "solstone-core-journal-io" "windows_atomic_detailed" "refs_managed_log_reference_receipt" "JOURNAL_WIN_CI_REFS_MANAGED_LOG_REFERENCE" "ReFS" || exit /b 1
call :run_receipt "NTFS stale-heartbeat cleanup" "solstone-core-system" "windows_lifecycle_receipt" "ntfs_stale_heartbeat_cleanup_receipt" "JOURNAL_WIN_CI_NTFS_STALE_HEARTBEAT_CLEANUP" "NTFS" || exit /b 1
call :run_receipt "ReFS stale-heartbeat cleanup" "solstone-core-system" "windows_lifecycle_receipt" "refs_stale_heartbeat_cleanup_receipt" "JOURNAL_WIN_CI_REFS_STALE_HEARTBEAT_CLEANUP" "ReFS" || exit /b 1
echo === cargo test --locked (journal-io library) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --lib || exit /b 1
echo === cargo test --locked (journal-io health-marker protocol) ===
set "JOURNAL_WIN_CI_HEALTH_MARKER_LOG=core\target\journal-win-ci-health-marker-%RANDOM%%RANDOM%.log"
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test windows_health_marker_protocol -- --nocapture > "%JOURNAL_WIN_CI_HEALTH_MARKER_LOG%" 2>&1
if not "%ERRORLEVEL%"=="0" ( del /q "%JOURNAL_WIN_CI_HEALTH_MARKER_LOG%" >nul 2>&1 & echo ERROR: journal-io health-marker protocol failed & exit /b 1 )
powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_HEALTH_MARKER_LOG); $marker = 'JOURNAL_WIN_CI_HEALTH_MARKER'; $pass = [regex]::Escape($marker + '=read/bump/lock/publish/legacy/pair/pass'); if ([regex]::Matches($text, '(?m)^' + [regex]::Escape($marker) + '=.*\r?$').Count -eq 1 -and [regex]::Matches($text, '(?m)^' + $pass + '\r?$').Count -eq 1) { exit 0 }; exit 1"
if not "%ERRORLEVEL%"=="0" ( del /q "%JOURNAL_WIN_CI_HEALTH_MARKER_LOG%" >nul 2>&1 & echo ERROR: journal-io health-marker protocol did not emit exactly one source-originated pass marker & exit /b 1 )
type "%JOURNAL_WIN_CI_HEALTH_MARKER_LOG%"
del /q "%JOURNAL_WIN_CI_HEALTH_MARKER_LOG%" >nul 2>&1
echo === cargo test --locked (journal-io snapshot protocol) ===
set "JOURNAL_WIN_CI_SNAPSHOT_LOG=core\target\journal-win-ci-snapshot-%RANDOM%%RANDOM%.log"
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test windows_snapshot_protocol -- --nocapture > "%JOURNAL_WIN_CI_SNAPSHOT_LOG%" 2>&1
if not "%ERRORLEVEL%"=="0" ( del /q "%JOURNAL_WIN_CI_SNAPSHOT_LOG%" >nul 2>&1 & echo ERROR: journal-io snapshot protocol failed & exit /b 1 )
powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_SNAPSHOT_LOG); $marker = 'JOURNAL_WIN_CI_SNAPSHOT'; $pass = [regex]::Escape($marker + '=capture/restore/reparse/pass'); if ([regex]::Matches($text, '(?m)^' + [regex]::Escape($marker) + '=.*\r?$').Count -eq 1 -and [regex]::Matches($text, '(?m)^' + $pass + '\r?$').Count -eq 1) { exit 0 }; exit 1"
if not "%ERRORLEVEL%"=="0" ( del /q "%JOURNAL_WIN_CI_SNAPSHOT_LOG%" >nul 2>&1 & echo ERROR: journal-io snapshot protocol did not emit exactly one source-originated pass marker & exit /b 1 )
type "%JOURNAL_WIN_CI_SNAPSHOT_LOG%"
del /q "%JOURNAL_WIN_CI_SNAPSHOT_LOG%" >nul 2>&1
echo === cargo test --locked (journal-io staged-directory protocol) ===
set "JOURNAL_WIN_CI_STAGED_LOG=core\target\journal-win-ci-staged-%RANDOM%%RANDOM%.log"
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test windows_staged_protocol --features test-hooks -- --test-threads=1 --nocapture > "%JOURNAL_WIN_CI_STAGED_LOG%" 2>&1
if not "%ERRORLEVEL%"=="0" ( type "%JOURNAL_WIN_CI_STAGED_LOG%" & del /q "%JOURNAL_WIN_CI_STAGED_LOG%" >nul 2>&1 & echo ERROR: journal-io staged-directory protocol failed & exit /b 1 )
powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_STAGED_LOG); $marker = [regex]::Escape('JOURNAL_WIN_CI_STAGED=publish/race/crash/cleanup/pass'); $ntfs = '^JOURNAL_WIN_CI_STAGED_NTFS_OUTCOMES=empty:(replaced|refused)/file:(replaced|refused)/nonempty:(replaced|refused)\r?$'; $refs = '^JOURNAL_WIN_CI_STAGED_REFS_OUTCOMES=empty:(replaced|refused)/file:(replaced|refused)/nonempty:(replaced|refused)\r?$'; if ([regex]::Matches($text, '(?m)^JOURNAL_WIN_CI_STAGED=.*\r?$').Count -eq 1 -and [regex]::Matches($text, '(?m)^' + $marker + '\r?$').Count -eq 1 -and [regex]::Matches($text, '(?m)^JOURNAL_WIN_CI_STAGED_OS=.+\r?$').Count -eq 1 -and [regex]::Matches($text, '(?m)' + $ntfs).Count -eq 1 -and [regex]::Matches($text, '(?m)' + $refs).Count -eq 1) { exit 0 }; exit 1"
if not "%ERRORLEVEL%"=="0" ( type "%JOURNAL_WIN_CI_STAGED_LOG%" & del /q "%JOURNAL_WIN_CI_STAGED_LOG%" >nul 2>&1 & echo ERROR: journal-io staged-directory protocol did not emit the exact source-originated NTFS/ReFS receipt & exit /b 1 )
type "%JOURNAL_WIN_CI_STAGED_LOG%"
del /q "%JOURNAL_WIN_CI_STAGED_LOG%" >nul 2>&1
echo === cargo test --locked (journal-io lock component) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test journal_io_lock_component --features test-hooks || exit /b 1
echo === cargo test --locked (journal-io detailed atomic publication) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test windows_atomic_detailed --features test-hooks || exit /b 1
call :run_source_marked_target "signed Windows payload verifier" "solstone-core-distribution" "windows_payload" "test-fixture-pin" "journal_win_ci_windows_payload_marker" "JOURNAL_WIN_CI_TARGET_WINDOWS_PAYLOAD" || exit /b 1
call :run_source_marked_target "journal-io create-only publication" "solstone-core-journal-io" "windows_create_only" "" "journal_win_ci_windows_create_only_marker" "JOURNAL_WIN_CI_TARGET_WINDOWS_CREATE_ONLY" || exit /b 1
call :run_source_marked_target "journal-io create-only publication protocol" "solstone-core-journal-io" "windows_create_only_protocol" "test-hooks" "journal_win_ci_windows_create_only_protocol_marker" "JOURNAL_WIN_CI_TARGET_WINDOWS_CREATE_ONLY_PROTOCOL" || exit /b 1
call :run_source_marked_target "journal-io install-file publication" "solstone-core-journal-io" "windows_install_file" "" "journal_win_ci_windows_install_file_marker" "JOURNAL_WIN_CI_TARGET_WINDOWS_INSTALL_FILE" || exit /b 1
echo === cargo test --locked (journal-io install-file publication protocol) ===
set "JOURNAL_WIN_CI_INSTALL_PROTOCOL_LOG=core\target\journal-win-ci-install-protocol-%RANDOM%%RANDOM%.log"
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --test windows_install_file_protocol --features test-hooks -- --nocapture > "%JOURNAL_WIN_CI_INSTALL_PROTOCOL_LOG%" 2>&1
if not "%ERRORLEVEL%"=="0" ( del /q "%JOURNAL_WIN_CI_INSTALL_PROTOCOL_LOG%" >nul 2>&1 & echo ERROR: journal-io install-file publication protocol failed & exit /b 1 )
powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_INSTALL_PROTOCOL_LOG); $marker = 'JOURNAL_WIN_CI_INSTALL_FILE_PROTOCOL'; $pass = [regex]::Escape($marker + '=admission/retry/sharing/reconciliation/cleanup/uncertainty/pass'); if ([regex]::Matches($text, '(?m)^' + [regex]::Escape($marker) + '=.*\r?$').Count -eq 1 -and [regex]::Matches($text, '(?m)^' + $pass + '\r?$').Count -eq 1) { exit 0 }; exit 1"
if not "%ERRORLEVEL%"=="0" ( del /q "%JOURNAL_WIN_CI_INSTALL_PROTOCOL_LOG%" >nul 2>&1 & echo ERROR: journal-io install-file protocol did not emit exactly one source-originated pass marker & exit /b 1 )
type "%JOURNAL_WIN_CI_INSTALL_PROTOCOL_LOG%"
del /q "%JOURNAL_WIN_CI_INSTALL_PROTOCOL_LOG%" >nul 2>&1
call :run_source_marker "journal-io install-file publication protocol" "solstone-core-journal-io" "windows_install_file_protocol" "test-hooks" "journal_win_ci_windows_install_file_protocol_marker" "JOURNAL_WIN_CI_TARGET_WINDOWS_INSTALL_FILE_PROTOCOL" || exit /b 1
call :run_source_marked_target "journal-io operational-log namespace" "solstone-core-journal-io" "windows_oplog_namespace" "test-hooks" "journal_win_ci_windows_oplog_namespace_marker" "JOURNAL_WIN_CI_TARGET_WINDOWS_OPLOG_NAMESPACE" || exit /b 1
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
echo === JOURNAL_WIN_CI_OK: source-bound native Windows MSVC journal gate passed; launch preparation, Job ownership, and mandatory NTFS and ReFS receipt markers were emitted and validated from their child logs ===
exit /b 0

:ordinary_owner_failed
set "JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=failed"
echo ERROR: ordinary-owner inventory control did not both exit successfully and emit JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed
exit /b 1

:ordinary_owner_cleanup_failed
goto :ordinary_owner_failed

:run_platform_receipt
set "JOURNAL_WIN_CI_PLATFORM_LABEL=%~1"
set "JOURNAL_WIN_CI_PLATFORM_PACKAGE=%~2"
set "JOURNAL_WIN_CI_PLATFORM_TARGET=%~3"
set "JOURNAL_WIN_CI_PLATFORM_SELECTOR=%~4"
set "JOURNAL_WIN_CI_PLATFORM_MARKER=%~5"
echo === cargo test --locked %JOURNAL_WIN_CI_PLATFORM_LABEL% native receipt ===
set "JOURNAL_WIN_CI_PLATFORM_LOG=core\target\journal-win-ci-platform-receipt-%RANDOM%%RANDOM%.log"
cargo test --manifest-path core\Cargo.toml --locked -p %JOURNAL_WIN_CI_PLATFORM_PACKAGE% --test %JOURNAL_WIN_CI_PLATFORM_TARGET% --features test-hooks -- --ignored --exact %JOURNAL_WIN_CI_PLATFORM_SELECTOR% --nocapture > "%JOURNAL_WIN_CI_PLATFORM_LOG%" 2>&1
set "JOURNAL_WIN_CI_PLATFORM_STATUS=%ERRORLEVEL%"
type "%JOURNAL_WIN_CI_PLATFORM_LOG%"
if not "%JOURNAL_WIN_CI_PLATFORM_STATUS%"=="0" exit /b 1
powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_PLATFORM_LOG); $marker = [regex]::Escape($env:JOURNAL_WIN_CI_PLATFORM_MARKER); $pass = [regex]::Escape($env:JOURNAL_WIN_CI_PLATFORM_MARKER + '=executed/pass'); if ([regex]::Matches($text, '(?m)^' + $marker + '=.*\r?$').Count -eq 1 -and [regex]::Matches($text, '(?m)^' + $pass + '\r?$').Count -eq 1) { exit 0 }; exit 1"
if not "%ERRORLEVEL%"=="0" ( echo ERROR: %JOURNAL_WIN_CI_PLATFORM_LABEL% receipt did not emit exactly one source-originated pass marker & exit /b 1 )
del /q "%JOURNAL_WIN_CI_PLATFORM_LOG%" >nul 2>&1
exit /b 0

:run_receipt
set "JOURNAL_WIN_CI_RECEIPT_LABEL=%~1"
set "JOURNAL_WIN_CI_RECEIPT_PACKAGE=%~2"
set "JOURNAL_WIN_CI_RECEIPT_TARGET=%~3"
set "JOURNAL_WIN_CI_RECEIPT_SELECTOR=%~4"
set "JOURNAL_WIN_CI_RECEIPT_MARKER=%~5"
set "JOURNAL_WIN_CI_RECEIPT_FILESYSTEM=%~6"
echo === cargo test --locked %JOURNAL_WIN_CI_RECEIPT_LABEL% native receipt ===
set "JOURNAL_WIN_CI_RECEIPT_LOG=core\target\journal-win-ci-receipt-%RANDOM%%RANDOM%.log"
cargo test --manifest-path core\Cargo.toml --locked -p %JOURNAL_WIN_CI_RECEIPT_PACKAGE% --test %JOURNAL_WIN_CI_RECEIPT_TARGET% --features test-hooks -- --ignored --exact %JOURNAL_WIN_CI_RECEIPT_SELECTOR% --nocapture > "%JOURNAL_WIN_CI_RECEIPT_LOG%" 2>&1
set "JOURNAL_WIN_CI_RECEIPT_STATUS=%ERRORLEVEL%"
type "%JOURNAL_WIN_CI_RECEIPT_LOG%"
if not "%JOURNAL_WIN_CI_RECEIPT_STATUS%"=="0" exit /b 1
powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_RECEIPT_LOG); $marker = [regex]::Escape($env:JOURNAL_WIN_CI_RECEIPT_MARKER); $filesystemMarker = [regex]::Escape($env:JOURNAL_WIN_CI_RECEIPT_MARKER + '_FILESYSTEM'); $pass = [regex]::Escape($env:JOURNAL_WIN_CI_RECEIPT_MARKER + '=executed/pass'); $filesystem = [regex]::Escape($env:JOURNAL_WIN_CI_RECEIPT_MARKER + '_FILESYSTEM=' + $env:JOURNAL_WIN_CI_RECEIPT_FILESYSTEM); if ([regex]::Matches($text, '(?m)^' + $marker + '=.*\r?$').Count -eq 1 -and [regex]::Matches($text, '(?m)^' + $filesystemMarker + '=.*\r?$').Count -eq 1 -and [regex]::Matches($text, '(?m)^' + $pass + '\r?$').Count -eq 1 -and [regex]::Matches($text, '(?m)^' + $filesystem + '\r?$').Count -eq 1) { exit 0 }; exit 1"
if not "%ERRORLEVEL%"=="0" ( echo ERROR: %JOURNAL_WIN_CI_RECEIPT_LABEL% receipt did not emit exactly one source-originated pass and runtime-filesystem marker & exit /b 1 )
set "JOURNAL_WIN_CI_CORTEX_NAMESPACE_TOKEN="
if "%JOURNAL_WIN_CI_RECEIPT_MARKER%"=="JOURNAL_WIN_CI_CORTEX_USE_NTFS" set "JOURNAL_WIN_CI_CORTEX_NAMESPACE_TOKEN=NTFS"
if "%JOURNAL_WIN_CI_RECEIPT_MARKER%"=="JOURNAL_WIN_CI_CORTEX_USE_REFS" set "JOURNAL_WIN_CI_CORTEX_NAMESPACE_TOKEN=REFS"
if defined JOURNAL_WIN_CI_CORTEX_NAMESPACE_TOKEN powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_RECEIPT_LOG); $categories = @('CREATE_ADMIT', 'WRONG_KIND_REPARSE', 'RETAINED_ROOT', 'RETAINED_HEALTH', 'FAILURE_MAPPING', 'PRESERVATION', 'LOCK'); foreach ($category in $categories) { $marker = 'JOURNAL_WIN_CI_CORTEX_NAMESPACE_' + $env:JOURNAL_WIN_CI_CORTEX_NAMESPACE_TOKEN + '_' + $category; $pass = [regex]::Escape($marker + '=executed/pass'); $filesystemMarker = [regex]::Escape($marker + '_FILESYSTEM=' + $env:JOURNAL_WIN_CI_RECEIPT_FILESYSTEM); if ([regex]::Matches($text, '(?m)^' + $pass + '\r?$').Count -ne 1 -or [regex]::Matches($text, '(?m)^' + $filesystemMarker + '\r?$').Count -ne 1) { exit 1 } }; exit 0"
if not "%ERRORLEVEL%"=="0" ( echo ERROR: %JOURNAL_WIN_CI_RECEIPT_LABEL% receipt did not emit every Cortex namespace category exactly once & exit /b 1 )
del /q "%JOURNAL_WIN_CI_RECEIPT_LOG%" >nul 2>&1
exit /b 0

:require_journal_test
set "JOURNAL_WIN_CI_TEST=%~1"
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal --lib -- --exact "%JOURNAL_WIN_CI_TEST%" 2>&1 | findstr /c:"test result: ok. 1 passed;" >nul || ( echo ERROR: required journal test %JOURNAL_WIN_CI_TEST% is missing ignored or failed & exit /b 1 )
exit /b 0

:run_source_marked_target
echo === cargo test --locked (%~1) ===
if "%~4"=="" (
  cargo test --manifest-path core\Cargo.toml --locked -p "%~2" --test "%~3" || exit /b 1
) else (
  cargo test --manifest-path core\Cargo.toml --locked -p "%~2" --test "%~3" --features "%~4" || exit /b 1
)
call :run_source_marker "%~1" "%~2" "%~3" "%~4" "%~5" "%~6" || exit /b 1
exit /b 0

:run_source_marker
set "JOURNAL_WIN_CI_TARGET_LOG=core\target\journal-win-ci-target-%RANDOM%%RANDOM%.log"
if "%~4"=="" (
  cargo test --manifest-path core\Cargo.toml --locked -p "%~2" --test "%~3" "%~5" -- --ignored --exact --nocapture > "%JOURNAL_WIN_CI_TARGET_LOG%" 2>&1
) else (
  cargo test --manifest-path core\Cargo.toml --locked -p "%~2" --test "%~3" --features "%~4" "%~5" -- --ignored --exact --nocapture > "%JOURNAL_WIN_CI_TARGET_LOG%" 2>&1
)
if not "%ERRORLEVEL%"=="0" ( type "%JOURNAL_WIN_CI_TARGET_LOG%" & del /q "%JOURNAL_WIN_CI_TARGET_LOG%" >nul 2>&1 & echo ERROR: %~1 source-marker test failed & exit /b 1 )
set "JOURNAL_WIN_CI_TARGET_MARKER=%~6"
powershell -NoProfile -Command "$text = [IO.File]::ReadAllText($env:JOURNAL_WIN_CI_TARGET_LOG); $key = [regex]::Escape($env:JOURNAL_WIN_CI_TARGET_MARKER); $pass = [regex]::Escape($env:JOURNAL_WIN_CI_TARGET_MARKER + '=executed/pass'); if ([regex]::Matches($text, '(?m)^' + $key + '=.*\r?$').Count -eq 1 -and [regex]::Matches($text, '(?m)^' + $pass + '\r?$').Count -eq 1) { exit 0 }; exit 1"
if not "%ERRORLEVEL%"=="0" ( type "%JOURNAL_WIN_CI_TARGET_LOG%" & del /q "%JOURNAL_WIN_CI_TARGET_LOG%" >nul 2>&1 & echo ERROR: %~1 did not emit exactly one source-originated target marker & exit /b 1 )
type "%JOURNAL_WIN_CI_TARGET_LOG%"
del /q "%JOURNAL_WIN_CI_TARGET_LOG%" >nul 2>&1
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
