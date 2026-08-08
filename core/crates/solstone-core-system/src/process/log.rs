// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::Local;

const CHRONICLE_DIR: &str = "chronicle";

/// Per-day operational log writer with stable day and journal health links.
pub struct DailyLogWriter {
    journal_root: PathBuf,
    reference: String,
    name: String,
    pinned: bool,
    current_day: String,
    file: File,
}

impl DailyLogWriter {
    pub fn new(
        journal_root: impl Into<PathBuf>,
        reference: impl Into<String>,
        name: impl Into<String>,
        day: Option<String>,
    ) -> io::Result<Self> {
        let journal_root = journal_root.into();
        let reference = reference.into();
        let name = name.into();
        let pinned = day.is_some();
        let current_day = day.unwrap_or_else(current_day);
        let file = open_log(&journal_root, &current_day, &reference, &name)?;
        let writer = Self {
            journal_root,
            reference,
            name,
            pinned,
            current_day,
            file,
        };
        writer.update_symlinks()?;
        Ok(writer)
    }

    pub fn path(&self) -> PathBuf {
        log_path(
            &self.journal_root,
            &self.current_day,
            &self.reference,
            &self.name,
        )
    }

    /// Keep drain threads alive: rollover and write I/O errors are best effort.
    pub fn write(&mut self, message: &str) {
        if !self.pinned {
            let day_now = current_day();
            if day_now != self.current_day {
                // Open before closing: an open failure must leave old state usable.
                if let Ok(new_file) =
                    open_log(&self.journal_root, &day_now, &self.reference, &self.name)
                {
                    let old_file = std::mem::replace(&mut self.file, new_file);
                    self.current_day = day_now;
                    let _ = self.update_symlinks();
                    drop(old_file);
                }
            }
        }
        // Python intentionally swallows disk-full/write failures in output drains.
        let _ = self.file.write_all(message.as_bytes());
        let _ = self.file.flush();
    }

    fn update_symlinks(&self) -> io::Result<()> {
        let day_health = self
            .journal_root
            .join(CHRONICLE_DIR)
            .join(&self.current_day)
            .join("health");
        let filename = format!("{}_{}.log", self.reference, self.name);
        atomic_symlink(&day_health.join(format!("{}.log", self.name)), &filename)?;
        atomic_symlink(
            &self
                .journal_root
                .join("health")
                .join(format!("{}.log", self.name)),
            &format!("../{CHRONICLE_DIR}/{}/health/{filename}", self.current_day),
        )
    }
}

fn current_day() -> String {
    Local::now().format("%Y%m%d").to_string()
}

fn log_path(root: &Path, day: &str, reference: &str, name: &str) -> PathBuf {
    root.join(CHRONICLE_DIR)
        .join(day)
        .join("health")
        .join(format!("{reference}_{name}.log"))
}

fn open_log(root: &Path, day: &str, reference: &str, name: &str) -> io::Result<File> {
    let path = log_path(root, day, reference, name);
    fs::create_dir_all(path.parent().expect("log path has parent"))?;
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(unix)]
fn atomic_symlink(link: &Path, target: &str) -> io::Result<()> {
    use std::os::unix::fs::symlink;

    fs::create_dir_all(link.parent().expect("link has parent"))?;
    let temporary = link.with_extension(format!(
        "tmp{}_{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_file(&temporary);
    symlink(target, &temporary)?;
    fs::rename(&temporary, link).inspect_err(|_error| {
        let _ = fs::remove_file(&temporary);
    })
}

#[cfg(not(unix))]
fn atomic_symlink(_link: &Path, _target: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symlinks unsupported",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac19_rollover_open_failure_retains_old_handle_for_retry() {
        let root = std::env::temp_dir().join(format!(
            "solstone-system-log-rollover-{}",
            std::process::id()
        ));
        let mut writer = DailyLogWriter::new(&root, "ref", "process", Some("19990101".to_owned()))
            .expect("old-day writer");
        writer.pinned = false;
        let today = current_day();
        fs::create_dir_all(&root).expect("journal root");
        fs::write(root.join(CHRONICLE_DIR).join(&today), "not a directory")
            .expect("block new day directory");
        writer.write("old handle remains usable\n");
        assert_eq!(writer.current_day, "19990101");
        assert!(
            fs::read_to_string(writer.path())
                .expect("old log")
                .contains("old handle")
        );
        let _ = fs::remove_dir_all(root);
    }
}
