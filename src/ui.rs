use crate::app::{App, Focus, Phase};
use crate::hosts::Status;
use ratatui::prelude::*;
use ratatui::widgets::*;

pub fn draw(f: &mut Frame, app: &App) {
    let [main, help] = Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).areas(f.area());
    let [hosts_area, dir, cmd, gauge, running] = Layout::vertical([
        Constraint::Max(app.hosts.len() as u16 + 3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(3),
    ])
    .areas(main);
    hosts(f, app, hosts_area);
    input(f, app, Focus::Dir, "Instances directory", &app.dir, dir);
    input(f, app, Focus::Command, "Command  (<exe> <flags>, instance path is appended)", &app.command, cmd);
    progress(f, app, gauge);
    running_pane(f, app, running);

    let msg = if app.confirm_abort {
        "Abort and kill remote runs? y/n".to_string()
    } else if let Some(m) = &app.message {
        m.clone()
    } else if app.phase == Phase::Running {
        "q abort".into()
    } else {
        "Tab next pane  Enter start  |  hosts: ↑/↓ move, space select (idle), a all idle, r refresh, q quit".into()
    };
    f.render_widget(Paragraph::new(msg), help);
}

fn focus_style(app: &App, f: Focus) -> Style {
    if app.focus == f && app.phase == Phase::Setup {
        Style::new().fg(Color::Yellow)
    } else {
        Style::new()
    }
}

fn input(f: &mut Frame, app: &App, which: Focus, title: &str, text: &str, area: Rect) {
    let cur = if app.focus == which && app.phase == Phase::Setup { "▏" } else { "" };
    f.render_widget(
        Paragraph::new(format!("{text}{cur}")).block(Block::bordered().title(title).border_style(focus_style(app, which))),
        area,
    );
}

fn hosts(f: &mut Frame, app: &App, area: Rect) {
    let running = app.phase == Phase::Running;
    let rows = app.hosts.iter().map(|h| {
        let (txt, color, load, top) = if running && h.selected {
            let failed = app.failed_hosts.iter().any(|(ip, _)| ip == &h.ip);
            if failed {
                ("FAIL".to_string(), Color::Magenta, String::new(), String::new())
            } else {
                ("RUN".to_string(), Color::Yellow, String::new(), String::new())
            }
        } else {
            match &h.status {
                Status::Unknown => ("...".to_string(), Color::Gray, String::new(), String::new()),
                Status::Idle { load, top } => ("IDLE".into(), Color::Green, format!("{load:.2}"), top.clone()),
                Status::Busy { load, top } => ("BUSY".into(), Color::Red, format!("{load:.2}"), top.clone()),
                Status::Down(_) => ("DOWN".into(), Color::DarkGray, String::new(), String::new()),
            }
        };
        let mark = if h.selected { "[x]" } else { "[ ]" };
        let row = Row::new(vec![
            Cell::from(mark),
            Cell::from(h.ip.clone()),
            Cell::from(txt).style(Style::new().fg(color)),
            Cell::from(load),
            Cell::from(top),
        ]);
        row
    });
    let sel = app.hosts.iter().filter(|h| h.selected).count();
    let t = Table::new(
        rows,
        [Constraint::Length(4), Constraint::Length(16), Constraint::Length(6), Constraint::Length(6), Constraint::Min(8)],
    )
    .header(Row::new(["", "Host", "State", "Load", "Top process / info"]).style(Style::new().bold()))
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
    .block(Block::bordered().title(format!(" Minipcs - {sel} selected ")).border_style(focus_style(app, Focus::Hosts)));
    let mut state = TableState::default().with_selected((app.focus == Focus::Hosts).then_some(app.cursor));
    f.render_stateful_widget(t, area, &mut state);
}

fn progress(f: &mut Frame, app: &App, area: Rect) {
    let ratio = if app.total == 0 { 0.0 } else { app.done as f64 / app.total as f64 };
    f.render_widget(
        Gauge::default()
            .block(Block::bordered().title(" Progress "))
            .ratio(ratio.min(1.0))
            .label(format!("{}/{}  ({} non-zero exit)", app.done, app.total, app.failed)),
        area,
    );
}

fn running_pane(f: &mut Frame, app: &App, area: Rect) {
    let mut cur: Vec<_> = app.current.iter().collect();
    cur.sort_by(|a, b| a.0.cmp(b.0));
    let mut items: Vec<ListItem> = cur
        .iter()
        .map(|(ip, (inst, t))| ListItem::new(format!("{ip:<16} {inst}  {}s", t.elapsed().as_secs())))
        .collect();
    items.extend(
        app.failed_hosts
            .iter()
            .map(|(ip, err)| ListItem::new(format!("{ip:<16} FAILED: {err}")).style(Style::new().fg(Color::Magenta))),
    );
    f.render_widget(List::new(items).block(Block::bordered().title(" Running ")), area);
}
