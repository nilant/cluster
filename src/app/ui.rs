use super::{App, HostRun};
use crate::hosts::Status;
use ratatui::prelude::*;
use ratatui::widgets::*;

const COMMAND_TITLE: &str = "Command  (<exe> <flags>, instance path appended)";
const HELP: &str =
    "Tab next pane  Enter start  |  hosts: ↑/↓ PgUp/PgDn move, space select (idle), a all idle, r refresh, q quit";

impl App {
    pub fn draw(&mut self, f: &mut Frame) {
        let [main, help] = Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).areas(f.area());
        let [hosts, inputs, gauge, _] = Layout::vertical([
            Constraint::Max(self.hosts.len() as u16 + 3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .areas(main);
        let inputs: [Rect; 2] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(inputs);

        let table = self.hosts_table();
        self.table.select(Some(self.cursor)); // keeps the cursor row scrolled into view
        f.render_stateful_widget(table, hosts, &mut self.table);
        for (i, (area, title)) in inputs.into_iter().zip(["Instances directory", COMMAND_TITLE]).enumerate() {
            let cursor = if self.focused(i + 1) { "▏" } else { "" };
            f.render_widget(
                Paragraph::new(format!("{}{cursor}", self.inputs[i])).block(self.block(title, i + 1)),
                area,
            );
        }
        f.render_widget(self.gauge(), gauge);
        let msg = match &self.message {
            _ if self.confirm_abort => "Abort and kill remote runs? y/n",
            Some(m) => m,
            None if self.running() => "↑/↓ PgUp/PgDn scroll hosts  q abort",
            None => HELP,
        };
        f.render_widget(Paragraph::new(msg), help);
    }

    /// Whether pane `pane` (see `App::focus`) takes keyboard input right now.
    fn focused(&self, pane: usize) -> bool {
        self.focus == pane && !self.running()
    }

    fn block<'a>(&self, title: impl Into<Line<'a>>, pane: usize) -> Block<'a> {
        let border = if self.focused(pane) { Style::new().yellow() } else { Style::new() };
        Block::bordered().title(title).border_style(border)
    }

    fn hosts_table(&self) -> Table<'static> {
        let running = self.running();
        let rows = self.hosts.iter().map(|h| {
            let (state, color, load, info): (_, _, String, Cell) = match (&h.run, &h.status) {
                (Some(run), _) if running => match run {
                    HostRun::Starting => ("RUN", Color::Yellow, "".into(), Cell::from("starting...").dark_gray()),
                    HostRun::Running(inst, t) => {
                        ("RUN", Color::Yellow, "".into(), format!("{inst}  {}s", t.elapsed().as_secs()).into())
                    }
                    HostRun::Done => ("DONE", Color::Green, "".into(), "".into()),
                    HostRun::Failed(e) => ("FAIL", Color::Magenta, "".into(), Cell::from(e.clone()).magenta()),
                },
                (_, Status::Unknown) => ("...", Color::Gray, "".into(), "".into()),
                (_, Status::Idle { load, top }) => ("IDLE", Color::Green, format!("{load:.2}"), top.clone().into()),
                (_, Status::Busy { load, top }) => ("BUSY", Color::Red, format!("{load:.2}"), top.clone().into()),
                (_, Status::Down) => ("DOWN", Color::DarkGray, "".into(), "".into()),
            };
            let mark = if h.selected { "[x]" } else { "[ ]" };
            Row::new([mark.into(), h.ip.clone().into(), Cell::from(state).fg(color), load.into(), info])
        });
        let selected = self.hosts.iter().filter(|h| h.selected).count();
        // the cursor row is only highlighted when the arrow keys move it
        let highlight = if self.focus == 0 || running { Style::new().reversed() } else { Style::new() };
        use Constraint::{Length, Min};
        Table::new(rows, [Length(4), Length(16), Length(6), Length(6), Min(8)])
            .header(Row::new(["", "Host", "State", "Load", "Top process / running"]).bold())
            .row_highlight_style(highlight)
            .block(self.block(format!(" Hosts - {selected} selected "), 0))
    }

    fn gauge(&self) -> Gauge<'_> {
        let (done, total, failed) = self.run.as_ref().map_or((0, 0, 0), |r| (r.done, r.job.total(), r.failed));
        Gauge::default()
            .block(
                Block::bordered()
                    .title(" Progress ")
                    .title(Line::from(format!(" {failed} non-zero exit ")).right_aligned()),
            )
            .ratio(if total == 0 { 0.0 } else { (done as f64 / total as f64).min(1.0) })
            .label(format!("{done}/{total}"))
    }
}
