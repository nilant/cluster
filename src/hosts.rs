use std::process::Stdio;
use tokio::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Unknown,
    Idle { load: f64, top: String },
    Busy { load: f64, top: String },
    Down(String),
}

impl Status {
    pub fn is_idle(&self) -> bool {
        matches!(self, Status::Idle { .. })
    }
}

/// Parses a hosts file: one IP address per line, blank lines and `#` comments ignored.
/// Returns every problem found (with line numbers) if the file is malformed.
pub fn parse_hosts(text: &str) -> Result<Vec<String>, Vec<String>> {
    let mut hosts: Vec<String> = Vec::new();
    let mut errors = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let l = line.trim();
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

/// Parses output of: cat /proc/loadavg; nproc; ps top line
pub fn parse_probe(out: &str, threshold: f64) -> Option<Status> {
    let mut lines = out.lines();
    let load: f64 = lines.next()?.split_whitespace().next()?.parse().ok()?;
    let cores: f64 = lines.next()?.trim().parse().ok()?;
    let top = lines.next().unwrap_or("").trim().to_string();
    let per_core = load / cores.max(1.0);
    Some(if per_core < threshold {
        Status::Idle { load, top }
    } else {
        Status::Busy { load, top }
    })
}

pub fn target(user: &Option<String>, host: &str) -> String {
    match user {
        Some(u) => format!("{u}@{host}"),
        None => host.to_string(),
    }
}

pub fn ssh_cmd(target: &str) -> Command {
    let mut c = Command::new("ssh");
    c.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "StrictHostKeyChecking=accept-new", target]);
    c.kill_on_drop(true);
    c
}

pub async fn probe(user: Option<String>, host: String, threshold: f64) -> Status {
    let remote = "cat /proc/loadavg; nproc; ps -eo pcpu,comm --sort=-pcpu | sed -n 2p";
    let out = ssh_cmd(&target(&user, &host))
        .arg(remote)
        .stdin(Stdio::null())
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => parse_probe(&String::from_utf8_lossy(&o.stdout), threshold)
            .unwrap_or(Status::Down("bad probe output".into())),
        Ok(o) => Status::Down(
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .last()
                .unwrap_or("ssh failed")
                .to_string(),
        ),
        Err(e) => Status::Down(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_trim_and_skip_blank() {
        assert_eq!(parse_hosts("1.1.1.1\n\n# c\n2.2.2.2\t\n  \n::1\n").unwrap(), vec!["1.1.1.1", "2.2.2.2", "::1"]);
    }

    #[test]
    fn hosts_malformed() {
        let e = parse_hosts("1.1.1.1\n192.168.1.300\nfoo\n1.1.1.1\n").unwrap_err();
        assert_eq!(e.len(), 3);
        assert!(e[0].starts_with("line 2:"));
        assert!(e[2].contains("duplicate"));
    }

    #[test]
    fn probe_idle_and_busy() {
        let idle = parse_probe("0.05 0.1 0.1 1/200 99\n4\n 1.0 sshd\n", 0.25).unwrap();
        assert!(idle.is_idle());
        let busy = parse_probe("3.50 2.0 1.0 1/200 99\n4\n 99.0 solver\n", 0.25).unwrap();
        assert!(!busy.is_idle());
        assert!(parse_probe("garbage", 0.25).is_none());
    }
}
