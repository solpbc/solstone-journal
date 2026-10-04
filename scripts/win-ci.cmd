@echo off
:: SPDX-License-Identifier: AGPL-3.0-only
:: Copyright (c) 2026 sol pbc
::
:: First native Windows journal gate. It proves the source-bound MSVC transport,
:: the portable journal substrate, and the registry-selected Windows integration suites.
:: The ordinary-owner inventory control is mandatory; backup and Cloud Files stay opt-in.
:: The mandatory native receipt children prove NTFS and ReFS publication and
:: stale-heartbeat cleanup. Their source-originated output is captured and
:: validated from child logs; the runner never synthesizes receipt markers.
:: Keep this runner CRLF-materialized: cmd.exe label calls fail on LF-only rewrites.
setlocal enableextensions
cd /d "%~dp0.." || exit /b 1

set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
:: Match the lean code/full CI profile on fresh native hosts. These settings
:: omit incremental caches and debugger symbols; optimization, debug assertions,
:: package features and the selected test population remain unchanged. Pin here
:: because the invoking Make environment does not cross the SSH boundary.
set "CARGO_INCREMENTAL=0"
set "CARGO_PROFILE_DEV_DEBUG=0"
if not defined EXPECTED_JOURNAL_COMMIT ( echo ERROR: EXPECTED_JOURNAL_COMMIT is required; rerun through win-host-ci & exit /b 1 )
if not defined EXPECTED_JOURNAL_CARGO_LOCK_SHA256 ( echo ERROR: EXPECTED_JOURNAL_CARGO_LOCK_SHA256 is required; rerun through win-host-ci & exit /b 1 )
if not defined SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT ( echo ERROR: SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT is required; rerun through win-host-ci & exit /b 1 )
powershell -NoProfile -Command "if ($env:EXPECTED_JOURNAL_COMMIT -notmatch '^[0-9a-f]{40}$' -or $env:EXPECTED_JOURNAL_CARGO_LOCK_SHA256 -notmatch '^[0-9a-f]{64}$') { exit 1 }" || ( echo ERROR: source-binding values must be lowercase full commit and SHA-256 hex; rerun through win-host-ci & exit /b 1 )

call :verify_source_binding || exit /b 1

if not defined JOURNAL_WIN_CI_RUN_BACKUP set "JOURNAL_WIN_CI_RUN_BACKUP=0"
powershell -NoProfile -Command "if ($env:JOURNAL_WIN_CI_RUN_BACKUP -cnotmatch '^[01]$') { exit 1 }" || ( echo ERROR: JOURNAL_WIN_CI_RUN_BACKUP must be 0 or 1 & exit /b 1 )
set "JOURNAL_WIN_CI_BACKUP_EVIDENCE=not-run"

if not defined JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST set "JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST=0"
powershell -NoProfile -Command "if ($env:JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST -notmatch '^[01]$') { exit 1 }" || ( echo ERROR: JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST must be 0 or 1; rerun through win-host-ci & exit /b 1 )
set "JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=skipped"

if not defined JOURNAL_WIN_CI_RUN_RFDETR set "JOURNAL_WIN_CI_RUN_RFDETR=0"
powershell -NoProfile -Command "if ($env:JOURNAL_WIN_CI_RUN_RFDETR -cnotmatch '^[01]$') { exit 1 }" || ( echo ERROR: JOURNAL_WIN_CI_RUN_RFDETR must be 0 or 1 & exit /b 1 )

if not defined JOURNAL_WIN_CI_RUN_ONNX_BINDING set "JOURNAL_WIN_CI_RUN_ONNX_BINDING=0"
powershell -NoProfile -Command "if ($env:JOURNAL_WIN_CI_RUN_ONNX_BINDING -cnotmatch '^[01]$') { exit 1 }" || ( echo ERROR: JOURNAL_WIN_CI_RUN_ONNX_BINDING must be 0 or 1 & exit /b 1 )

if not defined SOLSTONE_JOURNAL_WIN_REFS_ROOT ( echo ERROR: SOLSTONE_JOURNAL_WIN_REFS_ROOT is required for mandatory ReFS receipts; rerun through win-host-ci & exit /b 1 )
powershell -NoProfile -Command "if ($env:SOLSTONE_JOURNAL_WIN_REFS_ROOT -notmatch '^[A-Za-z]:[\\/][A-Za-z0-9_. ()\\/:=-]*$') { exit 1 }" || ( echo ERROR: SOLSTONE_JOURNAL_WIN_REFS_ROOT must be a safe absolute Windows path; rerun through win-host-ci & exit /b 1 )

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" ( echo ERROR: vswhere not found at "%VSWHERE%" & exit /b 1 )
set "VSINSTALL="
for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSINSTALL=%%i"
if not defined VSINSTALL ( echo ERROR: VS Build Tools with VC.Tools.x86.x64 not found & exit /b 1 )
call "%VSINSTALL%\VC\Auxiliary\Build\vcvarsall.bat" x64 >nul || ( echo ERROR: vcvarsall failed & exit /b 1 )

:: The vendored FFmpeg build's `sh`/`make`-driven configure needs a POSIX
:: shell, GNU make and an x86 assembler, and `bindgen` needs libclang to read
:: FFmpeg's headers; this box carries none of the four ambiently
:: (core\distribution\builder-inputs.toml's own comment: "the native host has
:: MSVC, but deliberately does not carry MSYS2/GNU make, NASM ... as ambient
:: build state"). Exactly one step of this gate compiles FFmpeg -- the
:: runtime-component receipt near the end -- so a cache-miss run without them
:: fails after the whole gate has already run. Staging happens here instead.
::
:: `solstone-distribution acquire ffmpeg-windows-tools` fetches the four pinned
:: archives from this checkout's own pin table and sha256-verifies every byte it
:: writes, the same fetch path the controlled producer uses. An archive already
:: present with the right digest is kept, so a persistent box downloads each one
:: once. The bootstrap below re-verifies them against the same pins, stages them
:: the way core\distribution\windows-produce.ps1 does, and proves in this run's
:: own environment that sh, make and nasm resolve to the staged copies while cl
:: and link still resolve to the host toolchain. Nothing is written to this
:: box's persistent PATH: registry environment changes made through an SSH
:: session are not picked up by a later SSH session here.
echo === cargo build --locked (distribution recorder for the FFmpeg toolchain bootstrap) ===
cargo build --manifest-path core\Cargo.toml --locked -p solstone-core-distribution --bin solstone-distribution || exit /b 1
if not defined JOURNAL_WIN_CI_FFMPEG_TOOLS_ROOT set "JOURNAL_WIN_CI_FFMPEG_TOOLS_ROOT=%USERPROFILE%\sj-ffmpeg-tools"
if not defined JOURNAL_WIN_CI_FFMPEG_INPUT_ROOT set "JOURNAL_WIN_CI_FFMPEG_INPUT_ROOT=%JOURNAL_WIN_CI_FFMPEG_TOOLS_ROOT%\inputs"
set "JOURNAL_WIN_CI_FFMPEG_ENV=core\target\journal-win-ci-ffmpeg-environment-%RANDOM%%RANDOM%.cmd"
echo === acquiring the pinned FFmpeg build toolchain ===
core\target\debug\solstone-distribution.exe acquire ffmpeg-windows-tools --dest "%JOURNAL_WIN_CI_FFMPEG_INPUT_ROOT%" || ( echo ERROR: pinned FFmpeg build toolchain acquisition failed & exit /b 1 )
echo === staging the pinned FFmpeg build toolchain ===
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\win-ci-ffmpeg-tools.ps1 -Mode stage -RepositoryRoot "%CD%" -ToolsRoot "%JOURNAL_WIN_CI_FFMPEG_TOOLS_ROOT%" -InputRoot "%JOURNAL_WIN_CI_FFMPEG_INPUT_ROOT%" -Recorder "%CD%\core\target\debug\solstone-distribution.exe" -EnvironmentScript "%CD%\%JOURNAL_WIN_CI_FFMPEG_ENV%" || ( echo ERROR: pinned FFmpeg build toolchain staging failed & exit /b 1 )
call "%JOURNAL_WIN_CI_FFMPEG_ENV%" || ( echo ERROR: staged FFmpeg build toolchain environment could not be applied & exit /b 1 )
del /q "%JOURNAL_WIN_CI_FFMPEG_ENV%" >nul 2>&1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\win-ci-ffmpeg-tools.ps1 -Mode assert -RepositoryRoot "%CD%" -ToolsRoot "%JOURNAL_WIN_CI_FFMPEG_TOOLS_ROOT%" || ( echo ERROR: staged FFmpeg build toolchain did not verify in this run's environment & exit /b 1 )

echo === cargo build --locked (portable journal substrate and CI runner) ===
cargo build --manifest-path core\Cargo.toml --locked -p solstone-core-journal -p solstone-core-journal-config -p solstone-core-journal-io -p solstone-core-system -p solstone-core-win-owner-rail || exit /b 1
cargo build --manifest-path core\Cargo.toml --locked -p solstone-core-repository-contracts --bin solstone-ci || exit /b 1
echo === cargo test --locked (the Windows journal app) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-app || exit /b 1
echo === cargo test --locked (Windows local thinking runtime) ===
cargo build --manifest-path core\Cargo.toml --locked -p solstone-core-vulkan-probe --bin solstone-core-vulkan-probe || exit /b 1
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-vulkan-probe --bin solstone-core-vulkan-probe || exit /b 1
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-callosum --lib --features full-tests windows_native_tests || exit /b 1
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-system --lib --features full-tests provider_runtime || exit /b 1
echo === cargo test --locked (late material reaches an old or finished day) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-system --lib --features full-tests -- --exact daily_coverage::tests::late_raw_input_adopts_an_old_day_and_reopens_a_finished_one daily_coverage::tests::adoption_persists_closed_calendar_boundary_and_reconciles_derived_only_changes daily_coverage::tests::explicit_old_day_registration_survives_restart_and_finds_lost_wake_changes || exit /b 1
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-local --lib --features full-tests || exit /b 1
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-generate-wire --lib || exit /b 1
echo === cargo test --locked (Windows portal installer Job ownership) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-convey-shell --lib --features full-tests thinking_install || exit /b 1
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-convey-shell --lib --features full-tests thinking_install::windows::tests::native::windows_installer_job_receipt -- --exact --ignored --nocapture || exit /b 1
echo === cargo test --locked (confidential processing is not offered on Windows) ===
call :run_exact_library "solstone-core-spp-ratls" "nvattest_authority::tests::only_a_platform_with_a_verifier_target_offers_confidential_processing" || exit /b 1
call :run_exact_library "solstone-core-spp-ratls" "nvattest_authority::tests::windows_owners_are_held_back_even_with_a_verifier_target" || exit /b 1
call :run_exact_library "solstone-core-spp-ratls" "ratls::channel::tests::a_held_platform_opens_no_channel" || exit /b 1
call :run_exact_library "solstone-core-spp-ratls" "ratls::channel::tests::windows_refuses_every_channel_before_connecting" || exit /b 1
call :run_exact_library "solstone-core-brain" "presentation::tests::a_platform_with_no_hardware_check_says_so_on_every_brain_surface" || exit /b 1
call :run_exact_library "solstone-core-thinking" "brain::tests::this_platform_offers_confidential_processing_everywhere_but_windows" || exit /b 1
call :run_exact_library "solstone-core-thinking" "brain::tests::with_no_hardware_check_confidential_processing_reads_not_on_platform" || exit /b 1
call :run_exact_library "solstone-core-convey-shell" "thinking::tests::no_hardware_check_refuses_confidential_turn_on" "full-tests" || exit /b 1
call :run_exact_library "solstone-core-convey-shell" "thinking::tests::a_confidential_lane_turned_on_before_still_turns_off" "full-tests" || exit /b 1
echo === cargo test --locked (Windows paired-device door listens on this computer by default) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-convey-shell --lib --features full-tests local_network || exit /b 1
:: solstone-core legs build with the features every shipped journal carries
:: (core\distribution\shipped-core-features.txt), so they test what owners run
:: and share one dependency build with the agent-connector build below.
echo === cargo test --locked (Windows installer process identity) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core --bin solstone-core --features full-tests,journal-mcp-endpoint install_provider::tests::local_install_reaches_existing_lease_with_current_process_identity -- --exact --nocapture || exit /b 1

echo === cargo test --locked (portable journal config substrate) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-config --lib || exit /b 1
echo === cargo test --locked --no-run (solstone-core Windows library harness) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core --lib --features test-hooks,journal-mcp-endpoint --no-run || exit /b 1

echo === owner evidence exporter controls ===
powershell -NoProfile -File scripts\win-owner-evidence-tests.ps1 || exit /b 1

echo === cargo test --locked journal-io ordinary-owner inventory control ===
set "JOURNAL_WIN_CI_OWNER_RAIL=core\target\debug\solstone-core-win-owner-rail.exe"
set "JOURNAL_WIN_CI_OWNER_LEASE=C:\ProgramData\solstone\journal-win-owner-rail\ordinary-owner.lease.json"
"%JOURNAL_WIN_CI_OWNER_RAIL%" recover-held --lease "%JOURNAL_WIN_CI_OWNER_LEASE%" || goto :ordinary_owner_failed
"%JOURNAL_WIN_CI_OWNER_RAIL%" prepare --lease "%JOURNAL_WIN_CI_OWNER_LEASE%" --worktree "%CD%" --worker "%CD%\%JOURNAL_WIN_CI_OWNER_RAIL%" --expected-commit "%EXPECTED_JOURNAL_COMMIT%" --expected-lock "%EXPECTED_JOURNAL_CARGO_LOCK_SHA256%" --owner-account "%SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT%" --refs-root-env SOLSTONE_JOURNAL_WIN_REFS_ROOT || goto :ordinary_owner_failed
powershell -NoProfile -Command "$lease = Get-Content $env:JOURNAL_WIN_CI_OWNER_LEASE | ConvertFrom-Json; if ([string]::IsNullOrWhiteSpace($lease.nonce)) { exit 1 }; Set-Content -Encoding ascii -Path 'core\target\journal-win-ci-owner-nonce.cmd' -Value ('set JOURNAL_WIN_CI_PREPARED_OWNER_NONCE=' + $lease.nonce)" || goto :ordinary_owner_failed
call core\target\journal-win-ci-owner-nonce.cmd || goto :ordinary_owner_failed
del /q core\target\journal-win-ci-owner-nonce.cmd >nul 2>&1
if not defined JOURNAL_WIN_CI_PREPARED_OWNER_NONCE goto :ordinary_owner_failed
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
powershell -NoProfile -File scripts\win-owner-evidence.ps1 -LeasePath "%JOURNAL_WIN_CI_OWNER_LEASE%" -OutputPath "core\target\journal-win-owner-evidence.json" -ExpectedNonce "%JOURNAL_WIN_CI_PREPARED_OWNER_NONCE%" -ExpectedCommit "%EXPECTED_JOURNAL_COMMIT%" -ExpectedLock "%EXPECTED_JOURNAL_CARGO_LOCK_SHA256%" -ExpectedOwnerAccount "%SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT%" || goto :ordinary_owner_failed
"%JOURNAL_WIN_CI_OWNER_RAIL%" cleanup --lease "%JOURNAL_WIN_CI_OWNER_LEASE%" || goto :ordinary_owner_failed
del /q "%JOURNAL_WIN_CI_ORDINARY_OWNER_LOG%" >nul 2>&1
set "JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=passed"

echo === cargo test --locked (journal-io library) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal-io --lib || exit /b 1
echo === checking required journal portability tests ===
call :require_journal_test tests::config_strip_matches_python_control_whitespace || exit /b 1
call :require_journal_test tests::ensure_journal_dir_reports_non_directory_parent || exit /b 1
echo === cargo test --locked (journal library) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal --lib || exit /b 1
echo === cargo test --locked (ingest resolve: the write path paired apps upload through) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-ingest-resolve --lib || exit /b 1
echo === cargo test --locked (PDF import worker: signed-package resolution and argv) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-import-sources --lib document::windows_worker || exit /b 1
echo === cargo test --locked (imported files keep their source time: documents and images) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-import-sources --lib shared::windows_set_times_tests || exit /b 1
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-import-sources --lib image::tests::an_imported_image_keeps_its_bytes_and_source_time || exit /b 1
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-import-sources --test source_document_contracts document::ac8_document_preserves_owner_source_and_installs_private_mtime_copy -- --exact || exit /b 1

:: The agent connector: the journal binary with it compiled in, its audit
:: record, and both its routine and boundary suites. The boundary suite is
:: what proves no tool call is served before its admission record exists.
echo === cargo build --locked (journal with the agent connector) ===
cargo build --manifest-path core\Cargo.toml --locked -p solstone-core --bin solstone-core --features journal-mcp-endpoint || exit /b 1
echo === cargo test --locked (agent connector audit record) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-mcp-audit --lib || exit /b 1
echo === cargo test --locked (agent connector routine suite) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-mcp-endpoint --lib -- --test-threads=1 || exit /b 1
echo === cargo test --locked (agent connector boundary suite) ===
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-mcp-endpoint --lib --features full-tests -- --test-threads=1 || exit /b 1
echo JOURNAL_WIN_CI_MCP_ENDPOINT=executed/pass
echo === cargo test --locked (journal doctor library, platform checks included) ===
:: Every other leg compiles the doctor only as a dependency.
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-doctor --lib --features full-tests || exit /b 1

call :run_exact_library "solstone-core-system" "process::platform::launch_control::installed_task_controls::installed_entry_refuses_cross_variant_and_every_changed_action_field" || exit /b 1
call :run_exact_library "solstone-core-system" "process::platform::launch_control::installed_task_controls::installed_wire_refuses_hosted_fields_unknown_variant_and_file_grants" || exit /b 1
call :run_exact_library "solstone-core-system" "process::platform::launch_control::installed_task_controls::guard_extraction_refuses_every_partial_set_and_non_unicode_value" || exit /b 1
call :run_exact_library "solstone-core-journal-cli" "runner::installed_task_controls::installed_request_preserves_exact_public_action" || exit /b 1
call :run_exact_library "solstone-core-journal-cli" "runner::installed_task_controls::installed_request_refuses_partial_non_unicode_and_rewritten_action" || exit /b 1
call :run_exact_library "solstone-core-journal-cli" "runner::installed_task_controls::installed_forwarding_refuses_changed_argv_before_launch" || exit /b 1
call :run_exact_library "solstone-core-service-unit" "windows_action::tests::guarded_action_round_trips_nondefault_port_and_unicode_path" || exit /b 1
call :run_exact_library "solstone-core-service-unit" "windows_action::tests::rejects_missing_partial_duplicate_malformed_and_extra_guard_fields" || exit /b 1
call :run_exact_library "solstone-core-service-unit" "windows_action::tests::quoting_matches_independent_literal_vectors" || exit /b 1
call :run_exact_library "solstone-core-service-unit" "windows_task_readback::tests::accepts_scheduler_metadata_without_weakening_action_validation" || exit /b 1
call :run_exact_library "solstone-core-service-unit" "windows_task_readback::tests::utf16_saved_artifact_retains_complete_action" || exit /b 1
call :run_exact_library "solstone-core-service-unit" "windows_task_readback::tests::requires_unified_engine_and_exact_exec_identity" || exit /b 1
call :run_exact_library "solstone-core-service-unit" "windows_task_readback::tests::refuses_duplicate_actions_triggers_wrong_namespace_and_privilege" || exit /b 1

echo === running native Windows suite gate from registry ===
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\win-ci-registry-tests.ps1 || exit /b 1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\win-ci-registry.ps1 || exit /b 1

if not exist core\target\journal-win-ci-evidence-vars.cmd exit /b 1
call core\target\journal-win-ci-evidence-vars.cmd || exit /b 1
del /q core\target\journal-win-ci-evidence-vars.cmd >nul 2>&1

:: Detect another operator replacing the persistent checkout while Cargo ran.
:: The driver-side lock normally serializes this rail; this second check keeps
:: an out-of-band checkout from earning a source-bound success marker.
call :verify_source_binding || exit /b 1

echo JOURNAL_WIN_CI_HEAD=%JOURNAL_WIN_CI_HEAD%
echo JOURNAL_WIN_CI_CARGO_LOCK_SHA256=%JOURNAL_WIN_CI_CARGO_LOCK_SHA256%
echo JOURNAL_WIN_CI_BACKUP_EVIDENCE=%JOURNAL_WIN_CI_BACKUP_EVIDENCE%
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

:require_journal_test
set "JOURNAL_WIN_CI_TEST=%~1"
set "JOURNAL_WIN_CI_TEST_LOG=core\target\journal-win-ci-required-%RANDOM%%RANDOM%.log"
cargo test --manifest-path core\Cargo.toml --locked -p solstone-core-journal --lib -- --exact "%JOURNAL_WIN_CI_TEST%" --show-output > "%JOURNAL_WIN_CI_TEST_LOG%" 2>&1
set "JOURNAL_WIN_CI_TEST_EXIT=%ERRORLEVEL%"
type "%JOURNAL_WIN_CI_TEST_LOG%"
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-win-exact-result.ps1 -LogPath "%JOURNAL_WIN_CI_TEST_LOG%" -TestName "%JOURNAL_WIN_CI_TEST%" -TestExitCode %JOURNAL_WIN_CI_TEST_EXIT% || exit /b 1
exit /b 0

:run_exact_library
set "JOURNAL_WIN_CI_EXACT_LOG=core\target\journal-win-ci-library-%RANDOM%%RANDOM%.log"
set "JOURNAL_WIN_CI_EXACT_FEATURES="
if not "%~3"=="" set "JOURNAL_WIN_CI_EXACT_FEATURES=--features %~3"
cargo test --manifest-path core\Cargo.toml --locked -p "%~1" --lib %JOURNAL_WIN_CI_EXACT_FEATURES% -- --exact "%~2" --show-output > "%JOURNAL_WIN_CI_EXACT_LOG%" 2>&1
set "JOURNAL_WIN_CI_EXACT_EXIT=%ERRORLEVEL%"
type "%JOURNAL_WIN_CI_EXACT_LOG%"
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-win-exact-result.ps1 -LogPath "%JOURNAL_WIN_CI_EXACT_LOG%" -TestName "%~2" -TestExitCode %JOURNAL_WIN_CI_EXACT_EXIT% || exit /b 1
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
