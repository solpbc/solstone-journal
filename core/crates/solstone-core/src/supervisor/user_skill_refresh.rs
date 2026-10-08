// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use log::{Level, Log, Metadata, Record};
use solstone_core_user_skill::{
    bundled_user_skill_dir, refresh_installed_user_skills, set_user_skill_copy_fault,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);
thread_local! {
    static WARN_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct TestLogger;
impl Log for TestLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Warn
    }
    fn log(&self, record: &Record) {
        if record.level() == Level::Warn {
            WARN_COUNT.with(|count| count.set(count.get() + 1));
        }
    }
    fn flush(&self) {}
}

static INIT_LOGGER: std::sync::Once = std::sync::Once::new();

#[test]
#[ignore = "isolated startup subprocess helper"]
fn production_startup_child() {
    let Ok(role) = std::env::var("SOLSTONE_STARTUP_SKILL_HELPER") else {
        return;
    };
    use solstone_core_installation_identity::{
        ArtifactBindingEvidence, LegacyManifestEvidence, OwnerBase, PlatformTag,
        SetupAdmissionRequest, admit_setup, journal_token_from_path, root_token_from_path,
    };
    use solstone_core_system::lifecycle::DeclaredParent;

    let home = PathBuf::from(std::env::var_os("HOME").expect("isolated home"));
    let journal = PathBuf::from(std::env::var_os("SOLSTONE_JOURNAL").expect("isolated journal"));
    let root = crate::installation_context::identity_root_from_current_executable().unwrap();
    let admission = admit_setup(SetupAdmissionRequest {
        owner: OwnerBase::at_home(home, PlatformTag::current()).unwrap(),
        root_token: root_token_from_path(&root).unwrap(),
        journal_token: journal_token_from_path(&journal).unwrap(),
        journal_is_explicit: true,
        accept_prepared_retarget: false,
        legacy_manifest: LegacyManifestEvidence::Absent,
        artifacts: ArtifactBindingEvidence::Fresh,
    })
    .unwrap();
    drop(admission);

    init_test_logger();
    if role == "fault" {
        set_user_skill_copy_fault(Some(|| Err(std::io::Error::other("injected copy failure"))));
    }
    let mut wrong_parent = DeclaredParent::capture_current().unwrap().instance();
    wrong_parent.pid = std::process::id();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let outcome = runtime.block_on(super::host::run_hosted(
        &journal,
        solstone_core_cli::SupervisorOptions {
            port: 0,
            journal_override: None,
            no_daily: true,
            no_schedule: true,
            no_convey: true,
            no_cortex: true,
            no_spl: true,
            direct_port: None,
            hosted_parent: true,
        },
        Some(DeclaredParent::from_instance(wrong_parent)),
    ));
    assert!(
        matches!(
            outcome,
            super::host::SupervisorHostOutcome::Refused {
                reason: super::host::SupervisorBootRefusal::ParentLiveness(_)
            }
        ),
        "startup must reach parent admission after optional refresh: {outcome:?}"
    );
    if role == "fault" {
        assert_eq!(WARN_COUNT.with(std::cell::Cell::get), 1);
    }
}

fn run_production_startup(home: &Path, journal: &Path, fault: bool) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "supervisor::user_skill_refresh::production_startup_child",
            "--ignored",
            "--nocapture",
        ])
        .env("HOME", home)
        .env("SOLSTONE_JOURNAL", journal)
        .env(
            "SOLSTONE_STARTUP_SKILL_HELPER",
            if fault { "fault" } else { "normal" },
        )
        // Existing fixture seam avoids a sibling executable preflight; the
        // deliberate parent mismatch stops before any runtime child starts.
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_BINARY", "unused")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("startup subprocess timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "startup helper failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn production_startup_refreshes_then_leaves_matching_and_removed_skills_alone() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let target = home.join(".claude/skills/solstone");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("SKILL.md"), b"---\nname: solstone\n---\nold").unwrap();
    run_production_startup(&home, &journal, false);
    let bundled = bundled_user_skill_dir().unwrap();
    assert!(solstone_core_skill_state::user_skill_copy_matches(&bundled, &target).unwrap());
    let dir = fs::metadata(&target).unwrap();
    let file = fs::metadata(target.join("SKILL.md")).unwrap();
    run_production_startup(&home, &journal, false);
    let after_dir = fs::metadata(&target).unwrap();
    let after_file = fs::metadata(target.join("SKILL.md")).unwrap();
    assert_eq!(
        (dir.ino(), dir.mtime(), dir.mtime_nsec()),
        (after_dir.ino(), after_dir.mtime(), after_dir.mtime_nsec())
    );
    assert_eq!(
        (file.ino(), file.mtime(), file.mtime_nsec()),
        (
            after_file.ino(),
            after_file.mtime(),
            after_file.mtime_nsec()
        )
    );
    fs::remove_dir_all(&target).unwrap();
    run_production_startup(&home, &journal, false);
    assert!(!target.exists());
}

#[test]
fn production_startup_preserves_a_journal_nested_in_an_old_skill() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let target = home.join(".claude/skills/solstone");
    let journal = target.join("journal");
    fs::create_dir_all(&journal).unwrap();
    let old = b"---\nname: solstone\n---\nold";
    fs::write(target.join("SKILL.md"), old).unwrap();
    fs::write(journal.join("owner-material"), b"preserve me").unwrap();
    run_production_startup(&home, &journal, false);
    assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), old);
    assert_eq!(
        fs::read(journal.join("owner-material")).unwrap(),
        b"preserve me"
    );
}

#[test]
fn production_startup_retries_after_a_copy_failure_without_losing_old_entries() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let target = home.join(".claude/skills/solstone");
    fs::create_dir_all(target.join("empty")).unwrap();
    fs::create_dir_all(target.join("nested")).unwrap();
    let old = b"---\nname: solstone\n---\nold";
    fs::write(target.join("SKILL.md"), old).unwrap();
    fs::write(target.join("nested/extra"), b"old extra bytes").unwrap();
    let before = collect_all_paths(&target);
    run_production_startup(&home, &journal, true);
    assert_eq!(collect_all_paths(&target), before);
    assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), old);
    assert_eq!(
        fs::read(target.join("nested/extra")).unwrap(),
        b"old extra bytes"
    );
    run_production_startup(&home, &journal, false);
    assert!(
        solstone_core_skill_state::user_skill_copy_matches(
            &bundled_user_skill_dir().unwrap(),
            &target
        )
        .unwrap()
    );
}

fn init_test_logger() {
    INIT_LOGGER.call_once(|| {
        let _ = log::set_boxed_logger(Box::new(TestLogger));
        log::set_max_level(log::LevelFilter::Warn);
    });
    WARN_COUNT.with(|count| count.set(0));
}

fn collect_all_paths(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                paths.extend(collect_all_paths(&p));
            }
            paths.push(p);
        }
    }
    paths.sort();
    paths
}

fn unique_temp(name: &str) -> PathBuf {
    let temp = std::env::temp_dir().join(format!(
        "solstone-refresh-test-{}-{}-{}",
        name,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp).expect("create temp dir");
    temp
}

fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create dst");
    for entry in fs::read_dir(src).expect("read src") {
        let entry = entry.expect("entry");
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_all(&path, &target);
        } else {
            fs::copy(&path, &target).expect("copy file");
        }
    }
}

#[test]
fn unix_startup_refresh_older_qualifying_tree() {
    let temp = unique_temp("older");
    let home = temp.join("home");
    let journal = temp.join("journal");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&journal).unwrap();

    let bundled = bundled_user_skill_dir().expect("bundled");
    let claude_skill = home.join(".claude/skills/solstone");
    copy_dir_all(&bundled, &claude_skill);

    // Modify one byte in a file while keeping frontmatter name: solstone
    let skill_file = claude_skill.join("SKILL.md");
    let mut content = fs::read_to_string(&skill_file).expect("read skill");
    content.push_str("\n<!-- modified -->\n");
    fs::write(&skill_file, content).expect("write modified");

    assert!(!solstone_core_skill_state::user_skill_copy_matches(&bundled, &claude_skill).unwrap());

    refresh_installed_user_skills(&home, &journal);

    assert!(solstone_core_skill_state::user_skill_copy_matches(&bundled, &claude_skill).unwrap());

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn unix_startup_refresh_second_call_preserves_inodes_and_mtimes() {
    let temp = unique_temp("idempotent");
    let home = temp.join("home");
    let journal = temp.join("journal");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&journal).unwrap();

    let bundled = bundled_user_skill_dir().expect("bundled");
    let claude_skill = home.join(".claude/skills/solstone");
    copy_dir_all(&bundled, &claude_skill);

    refresh_installed_user_skills(&home, &journal);

    let dir_meta = fs::metadata(&claude_skill).expect("dir meta");
    let skill_file = claude_skill.join("SKILL.md");
    let file_meta = fs::metadata(&skill_file).expect("file meta");

    let dir_ino = dir_meta.ino();
    let dir_mtime = dir_meta.mtime();
    let dir_mtime_nsec = dir_meta.mtime_nsec();
    let file_ino = file_meta.ino();
    let file_mtime = file_meta.mtime();
    let file_mtime_nsec = file_meta.mtime_nsec();

    refresh_installed_user_skills(&home, &journal);

    let dir_meta2 = fs::metadata(&claude_skill).expect("dir meta 2");
    let file_meta2 = fs::metadata(&skill_file).expect("file meta 2");

    assert_eq!(dir_meta2.ino(), dir_ino);
    assert_eq!(dir_meta2.mtime(), dir_mtime);
    assert_eq!(dir_meta2.mtime_nsec(), dir_mtime_nsec);
    assert_eq!(file_meta2.ino(), file_ino);
    assert_eq!(file_meta2.mtime(), file_mtime);
    assert_eq!(file_meta2.mtime_nsec(), file_mtime_nsec);

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn unix_startup_refresh_config_root_without_skill_stays_absent() {
    let temp = unique_temp("absent");
    let home = temp.join("home");
    let journal = temp.join("journal");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&journal).unwrap();

    // Create .claude without skills/solstone
    fs::create_dir_all(home.join(".claude")).unwrap();

    refresh_installed_user_skills(&home, &journal);

    assert!(!home.join(".claude/skills").exists());
    assert!(!home.join(".claude/skills/solstone").exists());

    // A removed skill also stays absent (skills/ exists but not solstone)
    fs::create_dir_all(home.join(".codex/skills")).unwrap();
    refresh_installed_user_skills(&home, &journal);
    assert!(!home.join(".codex/skills/solstone").exists());

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn unix_startup_refresh_preserve_no_recreate() {
    let temp = unique_temp("preserve");
    let home = temp.join("home");
    let journal = temp.join("journal");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&journal).unwrap();

    // 1. Regular file at skill path
    let claude_skills = home.join(".claude/skills");
    fs::create_dir_all(&claude_skills).unwrap();
    let file_target = claude_skills.join("solstone");
    fs::write(&file_target, b"regular_file_content").unwrap();

    // 2. Directory without SKILL.md
    let codex_skills = home.join(".codex/skills");
    let no_skill_md = codex_skills.join("solstone");
    fs::create_dir_all(&no_skill_md).unwrap();
    fs::write(no_skill_md.join("other.txt"), b"other").unwrap();

    // 3. Frontmatter name other than solstone
    let gemini_skills = home.join(".gemini/skills");
    let other_name = gemini_skills.join("solstone");
    fs::create_dir_all(&other_name).unwrap();
    fs::write(other_name.join("SKILL.md"), b"---\nname: other_tool\n---\n").unwrap();

    refresh_installed_user_skills(&home, &journal);

    assert_eq!(
        fs::read(&file_target).expect("read file"),
        b"regular_file_content"
    );
    assert!(!file_target.is_dir());
    assert!(!no_skill_md.join("SKILL.md").exists());
    assert_eq!(
        fs::read(other_name.join("SKILL.md")).expect("read other"),
        b"---\nname: other_tool\n---\n"
    );

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn unix_startup_refresh_unreadable_skill_directory() {
    use std::os::unix::fs::PermissionsExt;

    let temp = unique_temp("unreadable");
    let home = temp.join("home");
    let journal = temp.join("journal");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&journal).unwrap();

    let claude_skill = home.join(".claude/skills/solstone");
    fs::create_dir_all(&claude_skill).unwrap();
    fs::write(claude_skill.join("SKILL.md"), b"secret").unwrap();

    struct Restorer(PathBuf);
    impl Drop for Restorer {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }
    let _restorer = Restorer(claude_skill.clone());
    fs::set_permissions(&claude_skill, fs::Permissions::from_mode(0o000)).unwrap();

    init_test_logger();
    refresh_installed_user_skills(&home, &journal);

    assert!(WARN_COUNT.with(std::cell::Cell::get) >= 1);

    drop(_restorer);
    assert_eq!(fs::read(claude_skill.join("SKILL.md")).unwrap(), b"secret");

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn unix_startup_refresh_attributed_and_foreign_symlinks() {
    let temp = unique_temp("symlinks");
    let home = temp.join("home");
    let journal = temp.join("journal");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&journal).unwrap();

    let bundled = bundled_user_skill_dir().expect("bundled");
    let bundled_meta_before = fs::metadata(bundled.join("SKILL.md")).unwrap();
    let bundled_bytes_before = fs::read(bundled.join("SKILL.md")).unwrap();

    // 1. Attributed symlink pointing to bundled_user_skill_dir()
    let claude_skills = home.join(".claude/skills");
    fs::create_dir_all(&claude_skills).unwrap();
    let claude_target = claude_skills.join("solstone");
    std::os::unix::fs::symlink(&bundled, &claude_target).unwrap();

    // 2. Foreign symlink pointing elsewhere
    let foreign_dir = temp.join("foreign");
    fs::create_dir_all(&foreign_dir).unwrap();
    fs::write(foreign_dir.join("SKILL.md"), b"foreign").unwrap();
    let codex_skills = home.join(".codex/skills");
    fs::create_dir_all(&codex_skills).unwrap();
    let codex_target = codex_skills.join("solstone");
    std::os::unix::fs::symlink(&foreign_dir, &codex_target).unwrap();

    refresh_installed_user_skills(&home, &journal);

    // Claude symlink was replaced with a directory
    let claude_meta = fs::symlink_metadata(&claude_target).unwrap();
    assert!(!claude_meta.file_type().is_symlink());
    assert!(claude_meta.file_type().is_dir());
    assert!(solstone_core_skill_state::user_skill_copy_matches(&bundled, &claude_target).unwrap());

    // Bundled target bytes and mtime untouched
    let bundled_meta_after = fs::metadata(bundled.join("SKILL.md")).unwrap();
    let bundled_bytes_after = fs::read(bundled.join("SKILL.md")).unwrap();
    assert_eq!(bundled_bytes_before, bundled_bytes_after);
    assert_eq!(bundled_meta_before.mtime(), bundled_meta_after.mtime());
    assert_eq!(
        bundled_meta_before.mtime_nsec(),
        bundled_meta_after.mtime_nsec()
    );

    // Foreign symlink stays a symlink to foreign path
    let codex_meta = fs::symlink_metadata(&codex_target).unwrap();
    assert!(codex_meta.file_type().is_symlink());
    assert_eq!(fs::read_link(&codex_target).unwrap(), foreign_dir);

    let _ = fs::remove_dir_all(temp);
}

static FAULT_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
fn counting_copy_fault() -> std::io::Result<()> {
    if FAULT_CALL_COUNT.fetch_add(1, Ordering::SeqCst) == 0 {
        Err(std::io::Error::other("injected copy fault"))
    } else {
        Ok(())
    }
}

#[test]
fn unix_startup_refresh_fault_handling() {
    let temp = unique_temp("fault");
    let home = temp.join("home");
    let journal = temp.join("journal");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&journal).unwrap();

    let bundled = bundled_user_skill_dir().expect("bundled");

    // Both Claude and Codex eligible:
    // Claude has an older qualifying tree
    let claude_skill = home.join(".claude/skills/solstone");
    copy_dir_all(&bundled, &claude_skill);
    let claude_file = claude_skill.join("SKILL.md");
    fs::write(&claude_file, "---\nname: solstone\n---\n# Old Claude\n").unwrap();

    // Codex has an attributed link
    let codex_skills = home.join(".codex/skills");
    fs::create_dir_all(&codex_skills).unwrap();
    let codex_target = codex_skills.join("solstone");
    std::os::unix::fs::symlink(&bundled, &codex_target).unwrap();

    FAULT_CALL_COUNT.store(0, Ordering::SeqCst);
    set_user_skill_copy_fault(Some(counting_copy_fault));

    refresh_installed_user_skills(&home, &journal);

    // Claude encountered fault: prior bytes stay
    assert_eq!(
        fs::read_to_string(&claude_file).unwrap(),
        "---\nname: solstone\n---\n# Old Claude\n"
    );

    // Codex succeeded: first failure on Claude did not stop Codex
    let codex_meta = fs::symlink_metadata(&codex_target).unwrap();
    assert!(!codex_meta.file_type().is_symlink());
    assert!(codex_meta.file_type().is_dir());

    // Clear fault, later refresh on Claude succeeds
    set_user_skill_copy_fault(None);
    refresh_installed_user_skills(&home, &journal);

    assert!(solstone_core_skill_state::user_skill_copy_matches(&bundled, &claude_skill).unwrap());

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn unix_startup_refresh_overlap_and_selected_journal() {
    let temp = unique_temp("overlap");
    let home = temp.join("home");
    let journal = temp.join("journal");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&journal).unwrap();

    let marker = journal.join("marker.txt");
    fs::write(&marker, b"marker_data").unwrap();

    let bundled = bundled_user_skill_dir().expect("bundled");

    // 1. Skill nested in journal
    let nested_claude = journal.join(".claude");
    fs::create_dir_all(nested_claude.join("skills/solstone")).unwrap();
    let nested_file = nested_claude.join("skills/solstone/SKILL.md");
    fs::write(&nested_file, b"nested_old").unwrap();
    let home_claude = home.join(".claude");
    std::os::unix::fs::symlink(&nested_claude, &home_claude).unwrap();

    // 2. Legitimate skill outside journal: .codex
    let codex_skill = home.join(".codex/skills/solstone");
    copy_dir_all(&bundled, &codex_skill);
    fs::write(
        codex_skill.join("SKILL.md"),
        "---\nname: solstone\n---\n# Old Codex\n",
    )
    .unwrap();

    {
        refresh_installed_user_skills(&home, &journal);
    }

    // Overlap case: old bytes stay, marker bytes stay
    assert_eq!(fs::read(&nested_file).unwrap(), b"nested_old");
    assert_eq!(fs::read(&marker).unwrap(), b"marker_data");

    // Legitimate case: refreshes
    assert!(solstone_core_skill_state::user_skill_copy_matches(&bundled, &codex_skill).unwrap());

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn unix_startup_refresh_overlap_journal_nested_in_skill() {
    let temp = unique_temp("overlap_nested");
    let home = temp.join("home");
    fs::create_dir_all(&home).unwrap();

    let claude_skill = home.join(".claude/skills/solstone");
    fs::create_dir_all(&claude_skill).unwrap();
    let skill_file = claude_skill.join("SKILL.md");
    let old_bytes = b"---\nname: solstone\n---\n# Older Copy\n";
    fs::write(&skill_file, old_bytes).unwrap();

    // Selected journal is a directory inside skills/solstone
    let journal = claude_skill.join("journal");
    fs::create_dir_all(&journal).unwrap();
    let marker = journal.join("marker.txt");
    fs::write(&marker, b"marker_data").unwrap();

    let entries_before = collect_all_paths(&journal);

    {
        refresh_installed_user_skills(&home, &journal);
    }

    assert_eq!(fs::read(&skill_file).unwrap(), old_bytes);
    assert_eq!(fs::read(&marker).unwrap(), b"marker_data");
    let entries_after = collect_all_paths(&journal);
    assert_eq!(entries_before, entries_after);

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn unix_startup_refresh_overlap_same_directory() {
    let temp = unique_temp("overlap_same");
    let home = temp.join("home");
    fs::create_dir_all(&home).unwrap();

    // Selected journal is the skill directory itself
    let claude_skill = home.join(".claude/skills/solstone");
    let journal = claude_skill.clone();
    fs::create_dir_all(&claude_skill).unwrap();
    let skill_file = claude_skill.join("SKILL.md");
    let old_bytes = b"---\nname: solstone\n---\n# Older Copy\n";
    fs::write(&skill_file, old_bytes).unwrap();

    let marker = journal.join("marker.txt");
    fs::write(&marker, b"marker_data").unwrap();

    let entries_before = collect_all_paths(&journal);

    {
        refresh_installed_user_skills(&home, &journal);
    }

    assert_eq!(fs::read(&skill_file).unwrap(), old_bytes);
    assert_eq!(fs::read(&marker).unwrap(), b"marker_data");
    let entries_after = collect_all_paths(&journal);
    assert_eq!(entries_before, entries_after);

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn unix_startup_refresh_overlap_symlink_alias_into_journal() {
    let temp = unique_temp("overlap_alias");
    let home = temp.join("home");
    let journal = temp.join("journal");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&journal).unwrap();

    let marker = journal.join("marker.txt");
    fs::write(&marker, b"marker_data").unwrap();

    // Target inside journal has an eligible older copy
    let journal_skill_dir = journal.join("nested_skill");
    fs::create_dir_all(&journal_skill_dir).unwrap();
    let skill_file = journal_skill_dir.join("SKILL.md");
    let old_bytes = b"---\nname: solstone\n---\n# Older Copy\n";
    fs::write(&skill_file, old_bytes).unwrap();

    // skills/solstone is a symlink to a directory inside the journal
    let claude_skills = home.join(".claude/skills");
    fs::create_dir_all(&claude_skills).unwrap();
    let claude_link = claude_skills.join("solstone");
    std::os::unix::fs::symlink(&journal_skill_dir, &claude_link).unwrap();

    let entries_before = collect_all_paths(&journal);

    {
        refresh_installed_user_skills(&home, &journal);
    }

    // Link entry stays a symlink pointing to journal_skill_dir
    let link_meta = fs::symlink_metadata(&claude_link).unwrap();
    assert!(link_meta.file_type().is_symlink());
    assert_eq!(fs::read_link(&claude_link).unwrap(), journal_skill_dir);

    // Old skill bytes and marker stay
    assert_eq!(fs::read(&skill_file).unwrap(), old_bytes);
    assert_eq!(fs::read(&marker).unwrap(), b"marker_data");

    // No new file appears in the journal
    let entries_after = collect_all_paths(&journal);
    assert_eq!(entries_before, entries_after);

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn unix_startup_refresh_unknown_census() {
    use std::os::unix::fs::PermissionsExt;

    let temp = unique_temp("census");
    let home = temp.join("home");
    let journal = temp.join("journal");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&journal).unwrap();

    let bundled = bundled_user_skill_dir().expect("bundled");
    let claude_skill = home.join(".claude/skills/solstone");
    copy_dir_all(&bundled, &claude_skill);
    let skill_file = claude_skill.join("SKILL.md");
    fs::write(&skill_file, b"old_census_bytes").unwrap();

    let owner = solstone_core_installation_identity::OwnerBase::at_home(
        home.clone(),
        solstone_core_installation_identity::PlatformTag::current(),
    )
    .unwrap();
    fs::create_dir_all(owner.path()).unwrap();

    struct Restorer(PathBuf);
    impl Drop for Restorer {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }
    let _restorer = Restorer(owner.path());
    fs::set_permissions(owner.path(), fs::Permissions::from_mode(0o000)).unwrap();

    init_test_logger();
    refresh_installed_user_skills(&home, &journal);

    assert_eq!(WARN_COUNT.with(std::cell::Cell::get), 1);
    drop(_restorer);

    assert_eq!(fs::read(&skill_file).unwrap(), b"old_census_bytes");

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn unix_startup_refresh_warn_bound() {
    let temp = unique_temp("warn_bound");
    let home = temp.join("home");
    let journal = temp.join("journal");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&journal).unwrap();

    // Create .claude overlapping journal
    let claude_in_j = journal.join(".claude");
    fs::create_dir_all(claude_in_j.join("skills/solstone")).unwrap();
    std::os::unix::fs::symlink(&claude_in_j, home.join(".claude")).unwrap();

    init_test_logger();
    refresh_installed_user_skills(&home, &journal);

    // Exactly 1 warn for the single failed destination
    assert_eq!(WARN_COUNT.with(std::cell::Cell::get), 1);

    let _ = fs::remove_dir_all(temp);
}
