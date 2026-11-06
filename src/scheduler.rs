use crate::hosts::{ssh_cmd, target};
use crate::results::{self, Writer};
use anyhow::{bail, Result};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

pub enum Ev {
    Probe(usize, crate::hosts::Status),
    Start { host: String, instance: String },
    Result { host: String, code: i32 },
    HostFailed { host: String, err: String },
    WorkerEnd { host: String },
}

pub struct Shared {
    pub user: Option<String>,
    pub exe: PathBuf,
    pub flags: String,
    pub remote_dir: String,
    pub queue: Mutex<VecDeque<PathBuf>>,
    pub writer: Mutex<Writer>,
}

pub fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn list_instances(dir: &Path) -> Result<VecDeque<PathBuf>> {
    let rd = std::fs::read_dir(dir).map_err(|e| {
        let why = match e.kind() {
            std::io::ErrorKind::NotFound => "not found".to_string(),
            std::io::ErrorKind::NotADirectory => "not a directory".to_string(),
            _ => e.to_string(),
        };
        anyhow::anyhow!("instances directory {}: {why}", dir.display())
    })?;
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .collect();
    v.sort();
    Ok(v.into())
}

/// Key of an instance in the results file: the file name without its extension.
pub fn instance_key(path: &Path) -> String {
    path.file_stem().unwrap_or_default().to_string_lossy().into_owned()
}

fn exe_name(exe: &Path) -> String {
    exe.file_name().unwrap().to_string_lossy().into_owned()
}

async fn scp(tgt: &str, local: &Path, remote: &str) -> Result<()> {
    let o = Command::new("scp")
        .args(["-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "StrictHostKeyChecking=accept-new"])
        .arg(local)
        .arg(format!("{tgt}:{remote}"))
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    if !o.status.success() {
        bail!("scp failed: {}", String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(())
}

/// Runs a remote shell command; Err only on ssh-level failure (exit 255).
async fn remote(tgt: &str, cmd: &str) -> Result<(i32, String)> {
    let o = ssh_cmd(tgt).arg(cmd).stdin(Stdio::null()).output().await?;
    let code = o.status.code().unwrap_or(-1);
    if code == 255 {
        bail!("ssh: {}", String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok((code, String::from_utf8_lossy(&o.stdout).into_owned()))
}

pub async fn cleanup(sh: &Shared, host: &str) {
    let tgt = target(&sh.user, host);
    let pat = format!("[{}]{}/", &sh.remote_dir[..1], &sh.remote_dir[1..]);
    let cmd = format!("pkill -f {} ; rm -rf {}", sq(&pat), sq(&sh.remote_dir));
    let _ = ssh_cmd(&tgt).arg(cmd).stdin(Stdio::null()).output().await;
}

pub async fn worker(sh: Arc<Shared>, host: String, tx: UnboundedSender<Ev>) {
    if let Err(e) = work(&sh, &host, &tx).await {
        let _ = tx.send(Ev::HostFailed { host: host.clone(), err: e.to_string() });
    }
    cleanup(&sh, &host).await;
    let _ = tx.send(Ev::WorkerEnd { host: host.clone() });
}

async fn work(sh: &Shared, host: &str, tx: &UnboundedSender<Ev>) -> Result<()> {
    let tgt = target(&sh.user, host);
    let dir = &sh.remote_dir;
    let exe = exe_name(&sh.exe);
    remote(&tgt, &format!("mkdir -p {}", sq(dir))).await?;
    scp(&tgt, &sh.exe, &format!("{dir}/{exe}")).await?;
    remote(&tgt, &format!("chmod +x {}", sq(&format!("{dir}/{exe}")))).await?;

    loop {
        let Some(path) = sh.queue.lock().unwrap().pop_front() else { return Ok(()) };
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let res = run_one(sh, &tgt, &path, &name, host, tx).await;
        if let Err(e) = res {
            sh.queue.lock().unwrap().push_front(path); // requeue for another host
            return Err(e);
        }
    }
}

async fn run_one(sh: &Shared, tgt: &str, path: &Path, name: &str, host: &str, tx: &UnboundedSender<Ev>) -> Result<()> {
    let dir = &sh.remote_dir;
    let rpath = format!("{dir}/{name}");
    let _ = tx.send(Ev::Start { host: host.into(), instance: name.into() });
    scp(tgt, path, &rpath).await?;
    let exe = exe_name(&sh.exe);
    let cmd = format!("cd {} && ./{} {} {}; c=$?; rm -f {}; exit $c", sq(dir), sq(&exe), sh.flags, sq(name), sq(&rpath));
    let (code, stdout) = remote(tgt, &cmd).await?;
    results::append(&mut sh.writer.lock().unwrap(), &instance_key(path), stdout.trim_end_matches(['\n', '\r']))?;
    let _ = tx.send(Ev::Result { host: host.into(), code });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_drops_extension() {
        assert_eq!(instance_key(Path::new("d/012379a0-f981.txt")), "012379a0-f981");
        assert_eq!(instance_key(Path::new("d/a.tar.gz")), "a.tar");
        assert_eq!(instance_key(Path::new("d/noext")), "noext");
    }

    #[test]
    fn quote() {
        assert_eq!(sq("a'b"), "'a'\\''b'");
    }

    #[test]
    fn instances_sorted_skip_hidden() {
        let d = std::env::temp_dir().join(format!("cl-inst-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for n in ["b", "a", ".h"] {
            std::fs::write(d.join(n), "").unwrap();
        }
        let l = list_instances(&d).unwrap();
        assert_eq!(l.len(), 2);
        assert!(l[0].ends_with("a"));
        let _ = std::fs::remove_dir_all(d);
    }
}
