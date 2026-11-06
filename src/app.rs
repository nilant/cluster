mod ui;

use crate::{Args, hosts::Ssh, hosts::Status, scheduler::Ev, scheduler::Job};
use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use std::{sync::Arc, time::Duration, time::Instant};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

struct Host {
    ip: String,
    status: Status,
    selected: bool,
    /// state in the current or last run, if the host took part in it
    run: Option<HostRun>,
}

enum HostRun {
    Starting,
    Running(String, Instant),
    Done,
    Failed(String),
}

/// The current or last batch.
struct Run {
    job: Job,
    done: usize,
    failed: usize,
    active: usize,
}

pub struct App {
    args: Args,
    hosts: Vec<Host>,
    cursor: usize,
    table: TableState,
    /// 0 = hosts table, 1 = instances directory, 2 = command
    focus: usize,
    /// instances directory, command
    inputs: [String; 2],
    message: Option<String>,
    confirm_abort: bool,
    run: Option<Run>,
    refresh: Arc<Notify>,
    tx: UnboundedSender<Ev>,
    rx: UnboundedReceiver<Ev>,
}

impl App {
    /// Must be called inside the tokio runtime: it starts probing every host in the background.
    pub fn new(ips: Vec<String>, args: Args) -> Self {
        let (tx, rx) = unbounded_channel();
        let (refresh, ssh) = (Arc::new(Notify::new()), Ssh::new(args.user.as_deref()));
        for (i, ip) in ips.iter().enumerate() {
            let (ssh, ip, tx, refresh, threshold) =
                (ssh.clone(), ip.clone(), tx.clone(), refresh.clone(), args.idle_threshold);
            tokio::spawn(async move {
                while tx.send(Ev::Probe(i, ssh.probe(&ip, threshold).await)).is_ok() {
                    tokio::select! { _ = tokio::time::sleep(Duration::from_secs(5)) => {}, _ = refresh.notified() => {} }
                }
            });
        }
        let hosts =
            ips.into_iter().map(|ip| Host { ip, status: Status::Unknown, selected: false, run: None }).collect();
        App {
            args,
            hosts,
            cursor: 0,
            table: TableState::default(),
            focus: 0,
            inputs: Default::default(),
            message: None,
            confirm_abort: false,
            run: None,
            refresh,
            tx,
            rx,
        }
    }

    fn running(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.active > 0)
    }

    /// Applies the events sent by the background tasks since the last call.
    pub fn update(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Ev::Probe(i, status) => {
                    // a host that stops being idle is deselected, except during a run (it is busy with our work)
                    let keep = self.running() || status.is_idle();
                    let h = &mut self.hosts[i];
                    h.selected &= keep;
                    h.status = status;
                }
                Ev::Start(i, instance) => self.hosts[i].run = Some(HostRun::Running(instance, Instant::now())),
                Ev::Done(code) => {
                    if let Some(r) = &mut self.run {
                        r.done += 1;
                        r.failed += usize::from(code != 0);
                    }
                }
                Ev::End(i, err) => {
                    self.hosts[i].run = Some(err.map_or(HostRun::Done, HostRun::Failed));
                    let failed_hosts = self.hosts.iter().filter(|h| matches!(h.run, Some(HostRun::Failed(_)))).count();
                    if let Some(r) = &mut self.run {
                        r.active -= 1;
                        if r.active == 0 {
                            self.message = Some(format!(
                                "Finished: {}/{} instances run ({} non-zero exit, {failed_hosts} host failures) -> {}",
                                r.done,
                                r.job.total(),
                                r.failed,
                                r.job.results().display()
                            ));
                        }
                    }
                }
            }
        }
    }

    /// Handles a key press; returns true when the program should quit.
    pub async fn key(&mut self, k: KeyEvent) -> bool {
        use KeyCode::*;
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == Char('c') {
            return true;
        }
        if matches!(k.code, Up | Down | PageUp | PageDown) && (self.focus == 0 || self.running()) {
            let last = self.hosts.len() - 1;
            self.cursor = match k.code {
                Up => self.cursor.saturating_sub(1),
                Down => (self.cursor + 1).min(last),
                PageUp => self.cursor.saturating_sub(10),
                _ => (self.cursor + 10).min(last),
            };
            return false;
        }
        if self.running() {
            match k.code {
                Char('q') => self.confirm_abort = true,
                Char('y') if self.confirm_abort => {
                    if let Some(r) = &self.run {
                        r.job.abort().await;
                    }
                    return true;
                }
                _ => self.confirm_abort = false,
            }
            return false;
        }
        self.message = None;
        match (self.focus, k.code) {
            (_, Tab) => self.focus = (self.focus + 1) % 3,
            (_, BackTab) => self.focus = (self.focus + 2) % 3,
            (_, Enter) => {
                if let Err(e) = self.start() {
                    self.message = Some(format!("{e:#}"));
                }
            }
            (0, Char('q')) => return true,
            (0, Char(' ')) => {
                let h = &mut self.hosts[self.cursor];
                h.selected = !h.selected && h.status.is_idle();
            }
            (0, Char('a')) => self.hosts.iter_mut().for_each(|h| h.selected = h.status.is_idle()),
            (0, Char('r')) => self.refresh.notify_waiters(),
            (f @ 1..=2, Backspace) => {
                self.inputs[f - 1].pop();
            }
            (f @ 1..=2, Char(c)) => self.inputs[f - 1].push(c),
            _ => {}
        }
        false
    }

    fn start(&mut self) -> Result<()> {
        let hosts: Vec<_> =
            self.hosts.iter().enumerate().filter(|(_, h)| h.selected).map(|(i, h)| (i, h.ip.clone())).collect();
        if hosts.is_empty() {
            bail!("select at least one idle host (space)");
        }
        let active = hosts.len();
        let job = Job::start(&self.args, &self.inputs[1], &self.inputs[0], hosts, &self.tx)?;
        for h in &mut self.hosts {
            h.run = h.selected.then_some(HostRun::Starting);
        }
        self.run = Some(Run { job, done: 0, failed: 0, active });
        Ok(())
    }
}
