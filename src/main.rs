mod ansi;
mod app;
mod blog;
mod browser;
mod cleanup;
mod cmd;
mod complete;
mod config;
mod deploy;
mod desktop;
mod filter_list;
mod form;
mod fuzzy;
mod git;
mod history;
mod install;
mod line;
mod locate;
mod markdown;
mod notes;
mod platform;
mod proc;
mod project;
mod projects;
mod release;
mod scaffold;
mod settings;
mod shell;
mod targets;
mod task;
#[cfg(test)]
mod testutil;
mod textarea;
mod ui;
mod watch;

use std::{
    env,
    io::{self, Write},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};

use anyhow::Result;
use ratatui::{
    DefaultTerminal, Terminal,
    backend::CrosstermBackend,
    crossterm::{
        cursor,
        event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
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
    let start = platform::canonicalize(&start)?;
    let (config, config_err) = Config::load();
    let mut app = App::new(start, config, config_err);

    let mut terminal = ratatui::init();
    // Pastes arrive whole, so the post editor doesn't continue lists on every pasted line.
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let result = run(&mut terminal, &mut app);
    let _ = execute!(io::stdout(), DisableBracketedPaste);
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

        // Redraw faster while a task streams output, the spinner turns or git status is due.
        let timeout = if app.busy() { 50 } else { 500 };
        if event::poll(Duration::from_millis(timeout))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Paste(text) => app.on_paste(&text),
                _ => {}
            }
        }

        if let Some(fg) = app.pending.take() {
            let output = run_foreground(terminal, &fg, &app.config.shell())?;
            app.after_foreground(&fg, output);
        }
    }
    app.shutdown();
    Ok(())
}

/// Hands the real terminal to a command (nano, an interactive program, a shell).
/// Returns its stdout when `cd_to_output` asked for it to be captured.
fn run_foreground(terminal: &mut DefaultTerminal, fg: &Foreground, shell: &shell::Shell) -> Result<Option<String>> {
    execute!(io::stdout(), DisableBracketedPaste)?;
    ratatui::restore();
    execute!(io::stdout(), cursor::Show)?;
    if fg.wait {
        println!("\x1b[2m$ {}\x1b[0m", fg.cmd.display(shell));
    }

    let (status, output) = match fg.cmd.launches(shell) {
        Ok(launches) => run_launches(launches, fg, fg.wait),
        Err(e) => (Err(e), None),
    };

    if fg.wait {
        match status {
            Ok(None) => print!("\n\x1b[32m✔ nothing to do\x1b[0m"),
            Ok(Some(s)) if s.success() => print!("\n\x1b[32m✔ exited 0\x1b[0m"),
            Ok(Some(s)) => print!("\n\x1b[31m✘ {}\x1b[0m", s.code().map_or("killed".into(), |c| format!("exit {c}"))),
            Err(e) => print!("\n\x1b[31m✘ {e}\x1b[0m"),
        }
        print!(" — press Enter to return to mainframe ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
    }

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Clear(ClearType::All), EnableBracketedPaste)?;
    // A fresh Terminal forces a full redraw; `terminal.clear()` would query the
    // cursor position, which some terminals answer slowly or not at all.
    *terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    Ok(output)
}

/// Runs the processes in the real terminal, one after another while they succeed. With
/// `cd_to_output`, stdout is captured (pickers like fzf draw on the terminal itself, so
/// that doesn't disturb them).
fn run_launches(
    launches: Vec<cmd::Launch>,
    fg: &Foreground,
    show_steps: bool,
) -> (io::Result<Option<std::process::ExitStatus>>, Option<String>) {
    let several = launches.len() > 1;
    let mut captured = fg.cd_to_output.then(String::new);
    let mut last = None;
    for (i, mut l) in launches.into_iter().enumerate() {
        if l.unless_exists.as_ref().is_some_and(|p| fg.cwd.join(p).exists()) {
            continue;
        }
        if show_steps && several && i > 0 {
            println!("\x1b[2m$ {}\x1b[0m", l.display);
        }
        l.command.current_dir(&fg.cwd);
        let status = match &mut captured {
            Some(out) => l.command.stderr(Stdio::inherit()).output().map(|o| {
                out.push_str(&String::from_utf8_lossy(&o.stdout));
                o.status
            }),
            None => l.command.status(),
        };
        match status {
            Ok(s) if s.success() => last = Some(s),
            other => return (other.map(Some), captured),
        }
    }
    // None: every step was skipped, which counts as success.
    (Ok(last), captured)
}
