mod app;
mod hosts;
mod results;
mod scheduler;

use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind};
use std::{path::PathBuf, time::Duration};

#[derive(Parser)]
pub struct Args {
    /// File with one host address per line (default: ./hosts.txt)
    #[arg(long)]
    hosts: Option<PathBuf>,
    /// SSH user (default: ssh config / current user)
    #[arg(long)]
    pub user: Option<String>,
    /// A host is idle if its 1-minute load average is below this value (0.5 = one core at 50%)
    #[arg(long, default_value_t = 0.5)]
    pub idle_threshold: f64,
    /// Append to an existing results file instead of overwriting it
    #[arg(long)]
    pub append: bool,
    /// Directory for result files
    #[arg(long, default_value = "results")]
    pub results_dir: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut app = app::App::new(hosts::load(args.hosts.as_deref())?, args);
    let mut term = ratatui::init();
    let res = async {
        loop {
            app.update();
            term.draw(|f| app.draw(f))?;
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(k) = event::read()?
                && k.kind == KeyEventKind::Press
                && app.key(k).await
            {
                return Ok(());
            }
        }
    }
    .await;
    ratatui::restore();
    res
}
