use crate::hosts::Status;
use crate::scheduler::Ev;
use ratatui::widgets::TableState;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

#[derive(PartialEq, Clone, Copy)]
pub enum Phase {
    Setup,
    Running,
}

/// Which widget has keyboard focus.
#[derive(PartialEq, Clone, Copy)]
pub enum Focus {
    Hosts,
    Dir,
    Command,
}

pub struct HostRow {
    pub ip: String,
    pub status: Status,
    pub selected: bool,
    pub probing: bool,
    pub last_probe: Option<Instant>,
}

pub struct App {
    pub phase: Phase,
    pub hosts: Vec<HostRow>,
    pub cursor: usize,
    pub dir: String,
    /// executable followed by its flags; the instance path is appended
    pub command: String,
    pub focus: Focus,
    pub message: Option<String>,
    // running state
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub current: HashMap<String, (String, Instant)>,
    pub failed_hosts: Vec<(String, String)>,
    /// hosts whose worker ended normally in the current run
    pub finished: HashSet<String>,
    pub table_state: TableState,
    pub active_workers: usize,
    pub results_path: String,
    pub confirm_abort: bool,
}

impl App {
    pub fn new(ips: Vec<String>) -> Self {
        App {
            phase: Phase::Setup,
            hosts: ips
                .into_iter()
                .map(|ip| HostRow { ip, status: Status::Unknown, selected: false, probing: false, last_probe: None })
                .collect(),
            cursor: 0,
            dir: String::new(),
            command: String::new(),
            focus: Focus::Hosts,
            message: None,
            total: 0,
            done: 0,
            failed: 0,
            current: HashMap::new(),
            failed_hosts: vec![],
            finished: HashSet::new(),
            table_state: TableState::default(),
            active_workers: 0,
            results_path: String::new(),
            confirm_abort: false,
        }
    }

    pub fn selected_ips(&self) -> Vec<String> {
        self.hosts.iter().filter(|h| h.selected).map(|h| h.ip.clone()).collect()
    }

    pub fn on_event(&mut self, ev: Ev) {
        match ev {
            Ev::Probe(i, st) => {
                if let Some(h) = self.hosts.get_mut(i) {
                    h.probing = false;
                    h.last_probe = Some(Instant::now());
                    if self.phase == Phase::Setup && !st.is_idle() {
                        h.selected = false;
                    }
                    h.status = st;
                }
            }
            Ev::Start { host, instance } => {
                self.current.insert(host, (instance, Instant::now()));
            }
            Ev::Result { host, code } => {
                self.current.remove(&host);
                self.done += 1;
                if code != 0 {
                    self.failed += 1;
                }
            }
            Ev::HostFailed { host, err } => {
                self.current.remove(&host);
                self.failed_hosts.push((host, err));
            }
            Ev::WorkerEnd { host } => {
                if !self.failed_hosts.iter().any(|(h, _)| h == &host) {
                    self.finished.insert(host);
                }
                self.active_workers = self.active_workers.saturating_sub(1);
                if self.active_workers == 0 {
                    self.phase = Phase::Setup;
                    self.current.clear();
                    for h in &mut self.hosts {
                        h.last_probe = None; // refresh statuses now that the run is over
                    }
                    self.message = Some(format!(
                        "Finished: {}/{} instances run ({} non-zero exit, {} host failures) -> {}",
                        self.done, self.total, self.failed, self.failed_hosts.len(), self.results_path
                    ));
                }
            }
        }
    }
}
