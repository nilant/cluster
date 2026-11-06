use crate::app::{App, Focus, Phase};
use crate::hosts::Status;
use ratatui::prelude::*;
use ratatui::widgets::*;

pub fn draw(f: &mut Frame, app: &mut App) {
    let [main, help] = Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).areas(f.area());
    let [hosts_area, inputs, gauge, _] = Layout::vertical([
        Constraint::Max(app.hosts.len() as u16 + 3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(main);
    let [dir, cmd] = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(inputs);

    let mut state = std::mem::take(&mut app.table_state);
    hosts(f, app, hosts_area, &mut state);
    app.table_state = state;
    input(f, app, Focus::Dir, "Instances directory", &app.dir, dir);
    input(f, app, Focus::Command, "Command  (<exe> <flags>, instance path appended)", &app.command, cmd);
    progress(f, app, gauge);

    let msg = if app.confirm_abort {
        "Abort and kill remote runs? y/n".to_string()
    } else if let Some(m) = &app.message {
        m.clone()
    } else if app.phase == Phase::Running {
        "↑/↓ PgUp/PgDn scroll hosts  q abort".into()
    } else {
        "Tab next pane  Enter start  |  hosts: ↑/↓ PgUp/PgDn move, space select (idle), a all idle, r refresh, q quit".into()
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

fn hosts(f: &mut Frame, app: &App, area: Rect, state: &mut TableState) {
    let running = app.phase == Phase::Running;
    let rows = app.hosts.iter().map(|h| {
        let (txt, color, load, info, info_color) = if running && h.selected {
            if let Some((_, err)) = app.failed_hosts.iter().find(|(ip, _)| ip == &h.ip) {
                ("FAIL", Color::Magenta, String::new(), err.clone(), Color::Magenta)
            } else if let Some((inst, t)) = app.current.get(&h.ip) {
                ("RUN", Color::Yellow, String::new(), format!("{inst}  {}s", t.elapsed().as_secs()), Color::Reset)
            } else if app.finished.contains(&h.ip) {
                ("DONE", Color::Green, String::new(), String::new(), Color::Reset)
            } else {
                ("RUN", Color::Yellow, String::new(), "starting...".into(), Color::DarkGray)
            }
        } else {
            match &h.status {
                Status::Unknown => ("...", Color::Gray, String::new(), String::new(), Color::Reset),
                Status::Idle { load, top } => ("IDLE", Color::Green, format!("{load:.2}"), top.clone(), Color::Reset),
                Status::Busy { load, top } => ("BUSY", Color::Red, format!("{load:.2}"), top.clone(), Color::Reset),
                Status::Down(_) => ("DOWN", Color::DarkGray, String::new(), String::new(), Color::Reset),
            }
        };
        Row::new(vec![
            Cell::from(if h.selected { "[x]" } else { "[ ]" }),
            Cell::from(h.ip.clone()),
            Cell::from(txt).style(Style::new().fg(color)),
            Cell::from(load),
            Cell::from(info).style(Style::new().fg(info_color)),
        ])
    });
    let sel = app.hosts.iter().filter(|h| h.selected).count();
    // Selection always follows the cursor so the table scrolls; it is only drawn highlighted
    // when the hosts pane has focus (or while running, when it is the only thing to scroll).
    let highlight = if app.focus == Focus::Hosts || running {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new()
    };
    let t = Table::new(
        rows,
        [Constraint::Length(4), Constraint::Length(16), Constraint::Length(6), Constraint::Length(6), Constraint::Min(8)],
    )
    .header(Row::new(["", "Host", "State", "Load", "Top process / running"]).style(Style::new().bold()))
    .row_highlight_style(highlight)
    .block(Block::bordered().title(format!(" Hosts - {sel} selected ")).border_style(focus_style(app, Focus::Hosts)));
    state.select(Some(app.cursor));
    f.render_stateful_widget(t, area, state);
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
