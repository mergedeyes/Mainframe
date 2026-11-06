mod ansi;
mod app;
mod browser;
mod config;
mod form;
mod git;
mod install;
mod project;
mod task;
mod ui;

use std::{
    env,
    io::{self, Write},
    path::PathBuf,
    process::Command,
    time::Duration,
};

use anyhow::Result;
use ratatui::{
    DefaultTerminal, Terminal,
    backend::CrosstermBackend,
    crossterm::{
        cursor,
        event::{self, Event, KeyEventKind},
        execute,
        terminal::{Clear, ClearType, EnterAlternateScreen, enable_raw_mode},
    },
};

use app::{App, Foreground};
use config::Config;

fn main() -> Result<()> {
    let start = match env::args().nth(1) {
        Some(arg) if arg == "-h" || arg == "--help" => {
            println!("usage: mainframe [DIR]\n\nPress ? inside for keys. Config: {}", Config::path().display());
            return Ok(());
        }
        Some(arg) => PathBuf::from(arg),
        None => env::current_dir()?,
    };
    let start = start.canonicalize()?;
    let (config, config_err) = Config::load();
    let mut app = App::new(start, config, config_err);

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();

    // cd-on-exit: shell wrappers can read this (see README).
    if let Some(file) = env::var_os("MAINFRAME_CWD_FILE") {
        let _ = std::fs::write(file, app.browser.cwd.as_os_str().as_encoded_bytes());
    }
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.quit {
        app.tick();
        terminal.draw(|f| ui::draw(f, app))?;

        // Redraw faster while a task streams output or the spinner turns.
        let timeout = if app.task_running() { 50 } else { 500 };
        if event::poll(Duration::from_millis(timeout))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                _ => {}
            }
        }

        if let Some(fg) = app.pending.take() {
            run_foreground(terminal, &fg)?;
            app.after_foreground(&fg);
        }
    }
    if let Some(task) = &mut app.task {
        task.kill();
    }
    Ok(())
}

/// Hands the real terminal to a command (nano, an interactive program, a shell).
fn run_foreground(terminal: &mut DefaultTerminal, fg: &Foreground) -> Result<()> {
    ratatui::restore();
    execute!(io::stdout(), cursor::Show)?;
    if fg.wait {
        println!("\x1b[2m$ {}\x1b[0m", fg.cmd);
    }

    let status = Command::new("sh").arg("-c").arg(&fg.cmd).current_dir(&fg.cwd).status();

    if fg.wait {
        match status {
            Ok(s) if s.success() => print!("\n\x1b[32m✔ exited 0\x1b[0m"),
            Ok(s) => print!("\n\x1b[31m✘ {}\x1b[0m", s.code().map_or("killed".into(), |c| format!("exit {c}"))),
            Err(e) => print!("\n\x1b[31m✘ {e}\x1b[0m"),
        }
        print!(" — press Enter to return to mainframe ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
    }

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Clear(ClearType::All))?;
    // A fresh Terminal forces a full redraw; `terminal.clear()` would query the
    // cursor position, which some terminals answer slowly or not at all.
    *terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    Ok(())
}
