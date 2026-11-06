use crate::{Args, hosts::Ssh, hosts::Status, results::Writer};
use anyhow::{Context, Result, bail};
use std::collections::{HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};

/// Messages from background tasks to the UI. `usize` is the host's index in the hosts list.
pub enum Ev {
    Probe(usize, Status),
    /// a host started running an instance
    Start(usize, String),
    /// an instance finished with this exit code
    Done(i32),
    /// a host's worker stopped: no work left, or the error that made the host drop out
    End(usize, Option<String>),
}

struct Shared {
    ssh: Ssh,
    exe: PathBuf,
    flags: String,
    remote_dir: String,
    queue: Mutex<VecDeque<PathBuf>>,
    writer: Mutex<Writer>,
}

/// A batch run: one worker per host, each pulling the next instance from a shared queue.
pub struct Job {
    shared: Arc<Shared>,
    workers: Vec<(String, JoinHandle<()>)>,
    total: usize,
    results: PathBuf,
}

impl Job {
    /// `command` is `<exe> <flags>`; every file in `dir` is run as `<exe> <flags> <file>` on `hosts`.
    pub fn start(
        args: &Args,
        command: &str,
        dir: &str,
        hosts: Vec<(usize, String)>,
        tx: &UnboundedSender<Ev>,
    ) -> Result<Self> {
        let command = command.trim();
        let (exe, flags) = command.split_once(char::is_whitespace).unwrap_or((command, ""));
        let exe = expand_home(exe);
        if !exe.is_file() {
            bail!("executable not found: {}", exe.display());
        }
        let dir = expand_home(dir.trim());
        let queue = list_instances(&dir)?;
        let results = results_path(&args.results_dir, &dir, &exe);
        let total = queue.len();
        let shared = Arc::new(Shared {
            ssh: Ssh::new(args.user.as_deref()),
            writer: Mutex::new(Writer::open(results.clone(), args.append)?),
            flags: flags.trim().into(),
            remote_dir: format!("/tmp/cluster-{}", std::process::id()),
            queue: Mutex::new(queue),
            exe,
        });
        let workers = hosts
            .into_iter()
            .map(|(i, host)| (host.clone(), tokio::spawn(worker(shared.clone(), i, host, tx.clone()))))
            .collect();
        Ok(Self { shared, workers, total, results })
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn results(&self) -> &Path {
        &self.results
    }

    /// Stops all workers and kills their remote runs.
    pub async fn abort(&self) {
        for (host, handle) in &self.workers {
            handle.abort();
            cleanup(&self.shared, host).await;
        }
    }
}

/// `<base>/<instances dir>/<exe>.json`, keeping only plain path components (no `/`, `.` or `..`).
fn results_path(base: &Path, dir: &Path, exe: &Path) -> PathBuf {
    let mut p = base.to_path_buf();
    p.extend(dir.components().filter(|c| matches!(c, Component::Normal(_))));
    p.join(format!("{}.json", file_name(exe)))
}

fn expand_home(s: &str) -> PathBuf {
    match (s.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => Path::new(&home).join(rest),
        _ => s.into(),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// Key of an instance in the results file: the file name without its extension.
fn instance_key(p: &Path) -> String {
    p.file_stem().unwrap_or_default().to_string_lossy().into_owned()
}

/// Regular, non-hidden files directly in `dir`, sorted, with distinct keys.
fn list_instances(dir: &Path) -> Result<VecDeque<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("instances directory {}", dir.display()))?
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.is_file() && !file_name(p).starts_with('.'))
        .collect();
    files.sort();
    if files.is_empty() {
        bail!("no instance files in {}", dir.display());
    }
    let mut keys = HashSet::new();
    if let Some(p) = files.iter().find(|p| !keys.insert(instance_key(p))) {
        bail!("instances share the same name once the extension is removed: {}", instance_key(p));
    }
    Ok(files.into())
}

/// Single-quotes `s` for the remote shell.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

async fn worker(sh: Arc<Shared>, i: usize, host: String, tx: UnboundedSender<Ev>) {
    let err = work(&sh, i, &host, &tx).await.err().map(|e| format!("{e:#}"));
    cleanup(&sh, &host).await;
    let _ = tx.send(Ev::End(i, err));
}

async fn work(sh: &Shared, i: usize, host: &str, tx: &UnboundedSender<Ev>) -> Result<()> {
    let exe = format!("{}/{}", sh.remote_dir, file_name(&sh.exe));
    sh.ssh.run(host, &format!("mkdir -p {}", sq(&sh.remote_dir))).await?;
    sh.ssh.copy(&sh.exe, host, &exe).await?;
    sh.ssh.run(host, &format!("chmod +x {}", sq(&exe))).await?;
    loop {
        let Some(path) = sh.queue.lock().unwrap().pop_front() else { return Ok(()) };
        if let Err(e) = run_one(sh, i, host, &path, tx).await {
            sh.queue.lock().unwrap().push_front(path); // leave it for another host
            return Err(e);
        }
    }
}

async fn run_one(sh: &Shared, i: usize, host: &str, path: &Path, tx: &UnboundedSender<Ev>) -> Result<()> {
    let name = file_name(path);
    let _ = tx.send(Ev::Start(i, name.clone()));
    let remote = format!("{}/{name}", sh.remote_dir);
    sh.ssh.copy(path, host, &remote).await?;
    let exe = file_name(&sh.exe);
    let (dir, flags) = (sq(&sh.remote_dir), &sh.flags);
    let cmd = format!("cd {dir} && ./{} {flags} {}; c=$?; rm -f {}; exit $c", sq(&exe), sq(&name), sq(&remote));
    let out = sh.ssh.run(host, &cmd).await?;
    sh.writer.lock().unwrap().add(&instance_key(path), String::from_utf8_lossy(&out.stdout).trim_end())?;
    let _ = tx.send(Ev::Done(out.status.code().unwrap_or(-1)));
    Ok(())
}

/// Kills what is still running in the remote work directory and removes it.
async fn cleanup(sh: &Shared, host: &str) {
    // "[/]tmp/..." matches the remote run's command line but not this pkill's own shell
    let d = &sh.remote_dir;
    let _ =
        sh.ssh.run(host, &format!("pkill -f {}; rm -rf {}", sq(&format!("[{}]{}/", &d[..1], &d[1..])), sq(d))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(sq("a'b"), "'a'\\''b'");
        assert_eq!(instance_key(Path::new("d/012379a0-f981.txt")), "012379a0-f981");
        assert_eq!(instance_key(Path::new("d/a.tar.gz")), "a.tar");
        assert_eq!(instance_key(Path::new("d/noext")), "noext");
        let (r, exe) = (Path::new("results"), Path::new("/x/bottom_left"));
        assert_eq!(results_path(r, Path::new("./data/test/"), exe), Path::new("results/data/test/bottom_left.json"));
        assert_eq!(results_path(r, Path::new("/abs/dir"), exe), Path::new("results/abs/dir/bottom_left.json"));
    }

    #[test]
    fn instances_sorted_hidden_skipped_keys_distinct() {
        let d = std::env::temp_dir().join(format!("cl-inst-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(list_instances(&d).is_err(), "empty directory");
        for n in ["b.txt", "a", ".h"] {
            std::fs::write(d.join(n), "").unwrap();
        }
        let l = list_instances(&d).unwrap();
        assert_eq!(l.len(), 2);
        assert!(l[0].ends_with("a"));
        std::fs::write(d.join("b.json"), "").unwrap();
        assert!(list_instances(&d).unwrap_err().to_string().contains(": b"));
        let _ = std::fs::remove_dir_all(d);
    }
}
