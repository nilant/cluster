mod app;
mod hosts;
mod results;
mod scheduler;
mod ui;

use anyhow::{bail, Result};
use app::{App, Focus, Phase};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use scheduler::{Ev, Shared};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::task::JoinHandle;

#[derive(Parser)]
struct Args {
    /// File with one host address per line (default: ./hosts.txt)
    #[arg(long)]
    hosts: Option<PathBuf>,
    /// SSH user (default: ssh config / current user)
    #[arg(long)]
    user: Option<String>,
    /// Idle if load1 / cores is below this value
    #[arg(long, default_value_t = 0.25)]
    idle_threshold: f64,
    /// Append to an existing results file instead of overwriting it
    #[arg(long)]
    append: bool,
    /// Directory for result files
    #[arg(long, default_value = "results")]
    results_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let hosts_path = match &args.hosts {
        Some(p) if !p.is_file() => bail!("hosts file not found: {}", p.display()),
        Some(p) => p.clone(),
        None => {
            let default = PathBuf::from("hosts.txt");
            if !default.is_file() {
                bail!(
                    "no hosts file specified and no hosts.txt found in the current directory ({}).\n\
                     Pass one with --hosts <file> (one address per line) or run from a directory containing hosts.txt.",
                    std::env::current_dir()?.display()
                );
            }
            default
        }
    };
    let ips = hosts::parse_hosts(&std::fs::read_to_string(&hosts_path)?).map_err(|errs| {
        anyhow::anyhow!("malformed hosts file {}:\n  {}", hosts_path.display(), errs.join("\n  "))
    })?;
    if ips.is_empty() {
        bail!("no hosts in {}", hosts_path.display());
    }
    let mut term = ratatui::init();
    let res = run(&mut term, args, ips).await;
    ratatui::restore();
    res
}

fn spawn_probes(app: &mut App, args: &Args, tx: &UnboundedSender<Ev>) {
    for (i, h) in app.hosts.iter_mut().enumerate() {
        let stale = h.last_probe.is_none_or(|t| t.elapsed() > Duration::from_secs(5));
        if !h.probing && stale {
            h.probing = true;
            let (tx, user, ip, th) = (tx.clone(), args.user.clone(), h.ip.clone(), args.idle_threshold);
            tokio::spawn(async move {
                let st = hosts::probe(user, ip, th).await;
                let _ = tx.send(Ev::Probe(i, st));
            });
        }
    }
}

fn start(app: &mut App, args: &Args, tx: &UnboundedSender<Ev>) -> Result<(Arc<Shared>, Vec<JoinHandle<()>>)> {
    let dir = app.dir.trim().to_string();
    let cmd = app.command.trim().to_string();
    let (exe, flags) = match cmd.split_once(char::is_whitespace) {
        Some((e, f)) => (e.to_string(), f.trim().to_string()),
        None => (cmd.clone(), String::new()),
    };
    let exe = PathBuf::from(shellexpand(&exe));
    if !exe.is_file() {
        bail!("executable not found: {}", exe.display());
    }
    let dir_path = PathBuf::from(shellexpand(&dir));
    let queue = scheduler::list_instances(&dir_path)?;
    if queue.is_empty() {
        bail!("no instance files in {dir}");
    }
    let csv = results_path(&args.results_dir, &dir_path, &exe);
    let writer = results::open(&csv, !args.append)?;
    app.csv_path = csv.display().to_string();
    app.total = queue.len();
    app.done = 0;
    app.failed = 0;
    app.message = None;
    app.current.clear();
    app.failed_hosts.clear();
    let sh = Arc::new(Shared {
        user: args.user.clone(),
        exe,
        flags,
        remote_dir: format!("/tmp/cluster-{}", std::process::id()),
        queue: Mutex::new(queue),
        writer: Mutex::new(writer),
    });
    let ips = app.selected_ips();
    app.active_workers = ips.len();
    let handles = ips.into_iter().map(|ip| tokio::spawn(scheduler::worker(sh.clone(), ip, tx.clone()))).collect();
    Ok((sh, handles))
}

/// results/<instances path>/<exe name>.csv  (e.g. data/test -> results/data/test/bottom_left.csv)
/// Only normal path components are kept, so `./`, `..` and a leading `/` are dropped.
fn results_path(results_dir: &Path, instances_dir: &Path, exe: &Path) -> PathBuf {
    let mut p = results_dir.to_path_buf();
    p.extend(instances_dir.components().filter_map(|c| match c {
        std::path::Component::Normal(n) => Some(n),
        _ => None,
    }));
    p.join(format!("{}.csv", exe.file_name().unwrap().to_string_lossy()))
}

fn shellexpand(s: &str) -> String {
    match (s.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(h)) => format!("{h}/{rest}"),
        _ => s.to_string(),
    }
}

async fn run(term: &mut ratatui::DefaultTerminal, args: Args, ips: Vec<String>) -> Result<()> {
    let mut app = App::new(ips);
    let (tx, mut rx) = unbounded_channel::<Ev>();
    let mut job: Option<(Arc<Shared>, Vec<JoinHandle<()>>)> = None;
    let mut last_tick = Instant::now() - Duration::from_secs(10);

    loop {
        while let Ok(ev) = rx.try_recv() {
            app.on_event(ev);
        }
        // probes are paused during a run so busy/selected hosts keep their state
        if app.phase == Phase::Setup && last_tick.elapsed() > Duration::from_secs(1) {
            last_tick = Instant::now();
            spawn_probes(&mut app, &args, &tx);
        }
        term.draw(|f| ui::draw(f, &app))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(k) = event::read()? else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            break;
        }
        if app.phase == Phase::Running {
            match (app.confirm_abort, k.code) {
                (false, KeyCode::Char('q')) => app.confirm_abort = true,
                (true, KeyCode::Char('y')) => {
                    if let Some((sh, handles)) = job.take() {
                        for h in &handles {
                            h.abort();
                        }
                        for ip in app.selected_ips() {
                            scheduler::cleanup(&sh, &ip).await;
                        }
                    }
                    break;
                }
                (true, _) => app.confirm_abort = false,
                _ => {}
            }
            continue;
        }

        // Setup phase
        app.message = None;
        match k.code {
            KeyCode::Tab => {
                app.focus = match app.focus {
                    Focus::Hosts => Focus::Dir,
                    Focus::Dir => Focus::Command,
                    Focus::Command => Focus::Hosts,
                }
            }
            KeyCode::BackTab => {
                app.focus = match app.focus {
                    Focus::Hosts => Focus::Command,
                    Focus::Dir => Focus::Hosts,
                    Focus::Command => Focus::Dir,
                }
            }
            KeyCode::Enter => {
                if app.selected_ips().is_empty() {
                    app.message = Some("select at least one idle host (space)".into());
                } else {
                    match start(&mut app, &args, &tx) {
                        Ok(j) => {
                            job = Some(j);
                            app.phase = Phase::Running;
                        }
                        Err(e) => app.message = Some(e.to_string()),
                    }
                }
            }
            code => match app.focus {
                Focus::Hosts => match code {
                    KeyCode::Char('q') => break,
                    KeyCode::Up => app.cursor = app.cursor.saturating_sub(1),
                    KeyCode::Down => app.cursor = (app.cursor + 1).min(app.hosts.len() - 1),
                    KeyCode::Char(' ') => {
                        let h = &mut app.hosts[app.cursor];
                        if h.status.is_idle() {
                            h.selected = !h.selected;
                        }
                    }
                    KeyCode::Char('a') => app.hosts.iter_mut().for_each(|h| h.selected = h.status.is_idle()),
                    KeyCode::Char('r') => app.hosts.iter_mut().for_each(|h| h.last_probe = None),
                    _ => {}
                },
                Focus::Dir | Focus::Command => {
                    let field = if app.focus == Focus::Dir { &mut app.dir } else { &mut app.command };
                    match code {
                        KeyCode::Backspace => {
                            field.pop();
                        }
                        KeyCode::Char(c) => field.push(c),
                        _ => {}
                    }
                }
            },
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_path_mirrors_instances_path() {
        let exe = Path::new("/x/bottom_left");
        let r = Path::new("results");
        let want = PathBuf::from("results/data/test/bottom_left.csv");
        assert_eq!(results_path(r, Path::new("data/test"), exe), want);
        assert_eq!(results_path(r, Path::new("./data/test/"), exe), want);
        assert_eq!(results_path(r, Path::new("/abs/dir"), exe), PathBuf::from("results/abs/dir/bottom_left.csv"));
    }
}
