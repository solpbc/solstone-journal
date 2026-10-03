// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct IsolatedHome {
    root: PathBuf,
}

impl IsolatedHome {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "solstone-core-config-journal-{name}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("home/.local/bin")).expect("create isolated home bin");
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn journal(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir(&path).expect("create temporary journal");
        path
    }

    fn write_managed_wrapper(&self, current: &Path) {
        let sol_bin = self.root.join("runtime/solstone");
        let wrapper = format!(
            "#!/bin/bash\n# solstone — managed by 'journal config'. Edits will be overwritten.\n# managed-version: 7\n: \"${{SOLSTONE_JOURNAL:={}}}\"\nexport SOLSTONE_JOURNAL\nSOL_BIN='{}'\n# Warn when pyproject.toml or uv.lock is newer than .installed.\n# Skipped silently if .installed is absent.\nREPO_ROOT=\"${{SOL_BIN%/.venv/bin/solstone}}\"\nif [ -f \"$REPO_ROOT/.installed\" ]; then\n  if [ \"$REPO_ROOT/pyproject.toml\" -nt \"$REPO_ROOT/.installed\" ] \\\n     || [ \"$REPO_ROOT/uv.lock\" -nt \"$REPO_ROOT/.installed\" ]; then\n    echo \"solstone: WARNING — venv is stale (pyproject.toml or uv.lock changed since last install). Run: cd $REPO_ROOT && make install\" >&2\n  fi\nfi\nif [ ! -x \"$SOL_BIN\" ]; then\n    printf 'solstone: venv binary missing or not executable: %s\\n' \"$SOL_BIN\" >&2\n    exit 127\nfi\nexec \"$SOL_BIN\" \"$@\"\n",
            current.display(),
            sol_bin.display(),
        );
        fs::write(self.home().join(".local/bin/solstone"), wrapper).expect("write managed wrapper");
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write_config(journal: &Path, contents: &str) {
    let config = journal.join("config");
    fs::create_dir_all(&config).expect("create journal config directory");
    fs::write(config.join("journal.json"), contents).expect("write journal config");
}

fn write_legacy_manifest(journal: &Path) {
    let health = journal.join("health");
    fs::create_dir_all(&health).expect("create journal health directory");
    fs::write(
        health.join("setup-state.json"),
        r#"{"schema_version":1,"started_at":"2026-01-01T00:00:00Z","completed_at":null,"mode":"non_interactive","args_resolved":{},"steps":[]}"#,
    )
    .expect("write legacy setup manifest");
}

fn owner_sentence(journal: &Path) -> String {
    let journal = fs::canonicalize(journal).unwrap_or_else(|_| journal.to_path_buf());
    format!(
        "your settings file at {} couldn't be read. your settings were not changed. repair the file or restore config/journal.json from a backup, then try again.",
        journal.join("config/journal.json").display()
    )
}

fn run_switch(home: &IsolatedHome, current: &Path, target: &Path) -> Output {
    home.write_managed_wrapper(current);
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["config", "journal"])
        .arg(target)
        .args(["--switch", "--yes"])
        .env("HOME", home.home())
        .env_remove("SOLSTONE_JOURNAL")
        .output()
        .expect("solstone-core config journal runs")
}

#[test]
fn config_journal_switch_reports_corrupt_current_but_creates_missing_target() {
    let home = IsolatedHome::new("switch");
    let corrupt = home.journal("corrupt");
    let inactive = home.journal("inactive");
    let corrupt_target = home.root.join("corrupt-target");
    let missing_target = home.root.join("missing-target");
    write_config(&corrupt, "{bad json");
    write_config(&inactive, r#"{"setup": {}}"#);
    write_legacy_manifest(&inactive);

    let refused = run_switch(&home, &corrupt, &corrupt_target);
    assert_eq!(refused.status.code(), Some(1));
    let stderr = String::from_utf8(refused.stderr).expect("stderr is UTF-8");
    assert!(
        stderr.contains(&owner_sentence(&corrupt)),
        "stderr:\n{stderr}"
    );
    assert!(!corrupt_target.exists());

    let created = run_switch(&home, &inactive, &missing_target);
    assert_eq!(
        created.status.code(),
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr),
    );
    assert!(created.stderr.is_empty());
    assert!(missing_target.is_dir());
}
