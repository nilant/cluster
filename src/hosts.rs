use anyhow::{Result, anyhow, bail};
use std::path::Path;
use std::process::{Output, Stdio};
use tokio::process::Command;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Unknown,
    Idle {
        load: f64,
        top: String,
    },
    Busy {
        load: f64,
        top: String,
    },
    Down,
}

impl Status {
    pub fn is_idle(&self) -> bool {
        matches!(self, Status::Idle { .. })
    }
}

/// Reads the hosts file (`./hosts.txt` if `path` is None) and validates it.
pub fn load(path: Option<&Path>) -> Result<Vec<String>> {
    let p = path.unwrap_or(Path::new("hosts.txt"));
    if !p.is_file() {
        match path {
            Some(_) => bail!("hosts file not found: {}", p.display()),
            None => bail!(
                "no hosts file specified and no hosts.txt found in the current directory ({}).\n\
                 Pass one with --hosts <file> (one address per line) or run from a directory containing hosts.txt.",
                std::env::current_dir()?.display()
            ),
        }
    }
    let ips = parse_hosts(&std::fs::read_to_string(p)?)
        .map_err(|e| anyhow!("malformed hosts file {}:\n  {}", p.display(), e.join("\n  ")))?;
    if ips.is_empty() {
        bail!("no hosts in {}", p.display());
    }
    Ok(ips)
}

/// One IP address per line, blank lines and `#` comments ignored.
/// Returns every problem found (with line numbers) if the file is malformed.
fn parse_hosts(text: &str) -> Result<Vec<String>, Vec<String>> {
    let (mut hosts, mut errors) = (Vec::<String>::new(), Vec::new());
    for (i, l) in text.lines().map(str::trim).enumerate() {
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        match l.parse::<std::net::IpAddr>() {
            Err(_) => errors.push(format!("line {}: '{l}' is not a valid IP address", i + 1)),
            Ok(_) if hosts.iter().any(|h| h == l) => errors.push(format!("line {}: duplicate address '{l}'", i + 1)),
            Ok(_) => hosts.push(l.to_string()),
        }
    }
    if errors.is_empty() { Ok(hosts) } else { Err(errors) }
}

/// Output of the probe: `/proc/loadavg`, then the busiest process that is not the probe itself.
const PROBE: &str =
    r#"cat /proc/loadavg; ps -eo pcpu,comm --sort=-pcpu | awk 'NR>1 && $2!="ps" && $2!="awk" {print; exit}'"#;

/// Idle if the 1-minute load average (not divided by the core count) is below `threshold`.
fn parse_probe(out: &str, threshold: f64) -> Option<Status> {
    let mut lines = out.lines();
    let load: f64 = lines.next()?.split_whitespace().next()?.parse().ok()?;
    // CPU% right-aligned in 4 columns ("99.9", " 100") so the process names line up
    let top = match lines.next().unwrap_or("").trim().split_once(char::is_whitespace) {
        Some((cpu, name)) => format!("{cpu:>4} {}", name.trim()),
        None => String::new(),
    };
    Some(if load < threshold { Status::Idle { load, top } } else { Status::Busy { load, top } })
}

/// Runs ssh/scp non-interactively with the same options (and user, if given).
#[derive(Clone)]
pub struct Ssh(Vec<String>);

impl Ssh {
    pub fn new(user: Option<&str>) -> Self {
        let mut opts: Vec<String> =
            ["BatchMode=yes", "ConnectTimeout=5", "StrictHostKeyChecking=accept-new"].map(String::from).into();
        opts.extend(user.map(|u| format!("User={u}")));
        Self(opts)
    }

    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.args(self.0.iter().flat_map(|o| ["-o", o])).stdin(Stdio::null()).kill_on_drop(true);
        c
    }

    /// Runs `cmd` on `host`. Fails only if ssh itself fails (exit status 255).
    pub async fn run(&self, host: &str, cmd: &str) -> Result<Output> {
        let o = self.command("ssh").arg(host).arg(cmd).output().await?;
        if o.status.code() == Some(255) {
            bail!("ssh: {}", String::from_utf8_lossy(&o.stderr).trim());
        }
        Ok(o)
    }

    pub async fn copy(&self, local: &Path, host: &str, remote: &str) -> Result<()> {
        // brackets keep IPv6 addresses (which contain ':') apart from the path
        let o = self.command("scp").arg("-q").arg(local).arg(format!("[{host}]:{remote}")).output().await?;
        if !o.status.success() {
            bail!("scp: {}", String::from_utf8_lossy(&o.stderr).trim());
        }
        Ok(())
    }

    pub async fn probe(&self, host: &str, threshold: f64) -> Status {
        match self.run(host, PROBE).await {
            Ok(o) if o.status.success() => {
                parse_probe(&String::from_utf8_lossy(&o.stdout), threshold).unwrap_or(Status::Down)
            }
            _ => Status::Down,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_file() {
        assert_eq!(parse_hosts("1.1.1.1\n\n# c\n2.2.2.2\t\n  \n::1\n").unwrap(), ["1.1.1.1", "2.2.2.2", "::1"]);
        let e = parse_hosts("1.1.1.1\n192.168.1.300\nfoo\n1.1.1.1\n").unwrap_err();
        assert_eq!(e.len(), 3);
        assert!(e[0].starts_with("line 2:"));
        assert!(e[2].contains("duplicate"));
    }

    #[test]
    fn probe_idle_busy_and_top_column() {
        let idle = parse_probe("0.05 0.1 0.1 1/200 99\n  3.9 sshd\n", 0.5).unwrap();
        assert_eq!(idle, Status::Idle { load: 0.05, top: " 3.9 sshd".into() });
        // one fully used core is busy no matter how many cores the machine has
        let busy = parse_probe("1.00 1.0 1.0 2/200 99\n 100 solver\n", 0.5).unwrap();
        assert_eq!(busy, Status::Busy { load: 1.0, top: " 100 solver".into() });
        assert!(!parse_probe("0.50 0.1 0.1 1/200 99\n99.9 x\n", 0.5).unwrap().is_idle());
        assert!(parse_probe("garbage", 0.5).is_none());
    }
}
