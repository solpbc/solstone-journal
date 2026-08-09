// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use nix::sys::stat::Mode;
use nix::unistd::mkfifo;
use serde_json::{Map, Value, json};

use crate::oracle::sha256_hex;

pub(crate) struct Bed {
    base: PathBuf,
    pub root: PathBuf,
}
impl Bed {
    pub(crate) fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("solstone-core-cogitate-tools-{stamp}"));
        let root = base.join("journal");
        fs::create_dir_all(&root).expect("create bed root");
        build(&root);
        Self { base, root }
    }
}
impl Drop for Bed {
    fn drop(&mut self) {
        let locked = self.root.join("probe/locked.md");
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o600));
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn write(root: &Path, rel: &str, contents: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, contents).expect("write");
}
fn build(root: &Path) {
    write(
        root,
        "chronicle/20260809/default/120000_60/audio.jsonl",
        b"{\"start\":0,\"text\":\"hello\"}\n{\"start\":1,\"text\":\"world\"}\n",
    );
    write(
        root,
        "chronicle/20260809/default/120000_60/notes.md",
        b"line one\nline two\nline three\n",
    );
    write(root, "facets/work.md", b"work facet\nsunlight here\n");
    write(
        root,
        "talents/partner/abc.jsonl",
        b"{\"event\":\"finish\"}\n",
    );
    write(root, ".git/config", b"[core]\n");
    write(root, ".cache/x.txt", b"cached\n");
    write(root, "node_modules/pkg.json", b"{}\n");
    write(root, ".venv/pyvenv.cfg", b"home=/usr\n");
    for name in [
        "id_rsa",
        "id_rsa.pub",
        ".env",
        ".env.local",
        "server.key",
        "key.pem",
        "my.credentials",
        "credentials",
        "credentials.json",
        "token.key",
        "api_secret.txt",
        "token.txt",
        "passwords.md",
        "secrets.yaml",
    ] {
        write(root, &format!("secrets/{name}"), b"SECRET\n");
    }
    write(
        root,
        "blob.bin",
        &(0_u8..=255).cycle().take(1024).collect::<Vec<_>>(),
    );
    write(root, "empty.txt", b"");
    mkfifo(&root.join("fifo"), Mode::S_IRUSR | Mode::S_IWUSR).expect("create fifo");
    let outside = root.parent().expect("bed parent").join("outside");
    fs::create_dir_all(&outside).expect("outside");
    fs::write(outside.join("leak.txt"), b"LEAKED\n").expect("leak");
    symlink(outside.join("leak.txt"), root.join("escape")).expect("escape link");
    symlink(root.join("facets/work.md"), root.join("inside_link")).expect("inside link");
    write(
        root,
        "probe/alpha.md",
        b"Sunlight on the water\nsunlight again\nplain line\n",
    );
    write(
        root,
        "probe/beta.txt",
        b"before line\nSUNLIGHT shouting\nafter line\n",
    );
    write(root, "probe/gamma.log", b"no match here\n");
    write(root, "probe/.hidden_probe.md", b"sunlight hidden\n");
    write(
        root,
        "probe/binary.dat",
        &(0_u8..=255).cycle().take(2048).collect::<Vec<_>>(),
    );
    for index in 0..40 {
        write(
            root,
            &format!("probe/bulk/row{index:03}.txt"),
            format!("needle {index}\nfiller\n").as_bytes(),
        );
    }
    let long = (0..2000)
        .map(|index| format!("needle line {index}\n"))
        .collect::<String>();
    write(root, "probe/long.txt", long.as_bytes());
    write(root, "probe/locked.md", b"cannot read me\n");
    fs::set_permissions(
        root.join("probe/locked.md"),
        fs::Permissions::from_mode(0o0),
    )
    .expect("lock probe");
    write(root, ".hidden.md", b"hidden\n");
    let big = (0..3000)
        .map(|index| format!("row {index}\n"))
        .collect::<String>();
    write(root, "big.txt", big.as_bytes());
    for index in 0..250 {
        write(root, &format!("many/f{index:03}.txt"), b"x\n");
    }
}

pub(crate) fn manifest(root: &Path) -> Vec<Value> {
    let mut paths = Vec::new();
    collect(root, &mut paths);
    paths.sort_by_key(|path| {
        path.strip_prefix(root)
            .expect("relative")
            .to_string_lossy()
            .to_string()
    });
    paths.into_iter().map(|path| entry(root, &path)).collect()
}
fn collect(current: &Path, paths: &mut Vec<PathBuf>) {
    for item in fs::read_dir(current).expect("read bed") {
        let path = item.expect("entry").path();
        paths.push(path.clone());
        if !path.is_symlink() && path.is_dir() {
            collect(&path, paths);
        }
    }
}
fn entry(root: &Path, path: &Path) -> Value {
    let rel = path
        .strip_prefix(root)
        .expect("relative")
        .to_string_lossy()
        .replace('\\', "/");
    let metadata = fs::symlink_metadata(path).expect("lstat");
    let kind = metadata.file_type();
    if kind.is_symlink() {
        let target = fs::read_link(path).expect("readlink");
        let resolved = fs::canonicalize(path).expect("bed links resolve");
        return json!({"path":rel,"type":"symlink","target":normalize_target(&target),"escapes_root":!resolved.starts_with(root)});
    }
    if kind.is_fifo() {
        return json!({"path":rel,"type":"fifo"});
    }
    if kind.is_dir() {
        return json!({"path":rel,"type":"dir"});
    }
    let mode = format!("0o{:o}", metadata.permissions().mode() & 0o777);
    let readable = metadata.permissions().mode() & 0o444 != 0;
    let mut row = Map::new();
    row.insert("path".to_owned(), Value::String(rel));
    row.insert("type".to_owned(), Value::String("file".to_owned()));
    row.insert("mode".to_owned(), Value::String(mode));
    row.insert("readable".to_owned(), Value::Bool(readable));
    if readable {
        let bytes = fs::read(path).expect("read file");
        row.insert("byte_length".to_owned(), json!(bytes.len()));
        row.insert("sha256".to_owned(), Value::String(sha256_hex(&bytes)));
    }
    Value::Object(row)
}
fn normalize_target(target: &Path) -> String {
    let text = target.to_string_lossy().replace('\\', "/");
    if text.ends_with("/journal/facets/work.md") {
        "<journal>/facets/work.md".to_owned()
    } else if text.ends_with("/outside/leak.txt") {
        "<parent>/outside/leak.txt".to_owned()
    } else {
        text
    }
}
pub(crate) fn normalize_expected(entries: &[Value]) -> Vec<Value> {
    entries
        .iter()
        .cloned()
        .map(|mut entry| {
            if let Some(target) = entry
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                let normalized = if target.ends_with("/journal/facets/work.md") {
                    "<journal>/facets/work.md"
                } else if target.ends_with("/outside/leak.txt") {
                    "<parent>/outside/leak.txt"
                } else {
                    &target
                };
                entry["target"] = Value::String(normalized.to_owned());
            }
            entry
        })
        .collect()
}
