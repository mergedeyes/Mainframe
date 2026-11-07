//! Output tabs per project. Moving into another project puts the current tabs away (their
//! tasks keep running and their output keeps coming in) and brings back the ones that
//! project had, so you see where you left off. Outside of any project the tabs stay.
//!
//! They're also kept on disk, colors included, in `$XDG_STATE_HOME/mainframe/output/`:
//! saved when you switch away, when a task finishes, when a tab closes and on quit,
//! read back the first time you enter the project again.

use std::{
    collections::HashMap,
    fs, io, mem,
    path::{Path, PathBuf},
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};

use super::{App, Focus, Tab, tasks::LastRun};
use crate::{ansi, config, project::Plan, task::Output};

/// Per tab, the newest lines kept on disk; enough to see what happened, small to save.
const SAVED_LINES: usize = 5000;
/// Bumped when the file format changes; older files are ignored.
const VERSION: u32 = 1;

pub struct Workspace {
    pub(super) tabs: Vec<Tab>,
    pub(super) tab: usize,
    pub(super) show_output: bool,
    pub(super) zoom_output: bool,
}

impl Default for Workspace {
    fn default() -> Self {
        Self { tabs: vec![Tab::default()], tab: 0, show_output: false, zoom_output: false }
    }
}

impl Workspace {
    pub(super) fn running(&self) -> usize {
        self.tabs.iter().filter(|t| t.running()).count()
    }

    fn save(&self, root: &Path) {
        save(root, &self.tabs, self.tab, self.show_output, self.zoom_output);
    }
}

/// Worth keeping: something ran or runs here.
fn used(tabs: &[Tab]) -> bool {
    tabs.iter().any(|t| t.running() || t.plan.is_some() || t.watch.is_some() || !t.output.lines.is_empty())
}

/// Put-away workspaces by project root.
pub type Workspaces = HashMap<PathBuf, Workspace>;

impl App {
    /// Shows `root`'s tabs, putting the current project's away.
    pub(super) fn switch_workspace(&mut self, root: PathBuf) {
        let Some(current) = self.workspace.replace(root.clone()) else {
            // The first project seen: what it showed last time, unless something ran already.
            if !used(&self.tabs)
                && let Some(ws) = load(&root)
            {
                self.swap_in(ws);
            }
            return;
        };
        if current == root {
            return;
        }
        let incoming = self.workspaces.remove(&root).or_else(|| load(&root)).unwrap_or_default();
        let outgoing = self.swap_in(incoming);
        outgoing.save(&current);
        if used(&outgoing.tabs) {
            self.workspaces.insert(current, outgoing);
        }
    }

    /// Makes `ws` the shown workspace and returns the one it replaces.
    fn swap_in(&mut self, ws: Workspace) -> Workspace {
        let old = Workspace {
            tabs: mem::replace(&mut self.tabs, ws.tabs),
            tab: mem::replace(&mut self.tab, ws.tab),
            show_output: mem::replace(&mut self.show_output, ws.show_output),
            zoom_output: mem::replace(&mut self.zoom_output, ws.zoom_output),
        };
        if !self.show_output {
            self.focus = Focus::Browser;
        }
        old
    }

    pub(super) fn save_current(&self) {
        if let Some(root) = &self.workspace {
            save(root, &self.tabs, self.tab, self.show_output, self.zoom_output);
        }
    }

    pub(super) fn save_workspace(&self, root: &Path) {
        if let Some(ws) = self.workspaces.get(root) {
            ws.save(root);
        }
    }

    /// On quit: stops every task of every project, notes that in its output, saves it all.
    pub fn shutdown(&mut self) {
        let hidden = self.workspaces.values_mut().flat_map(|w| w.tabs.iter_mut());
        let tabs: Vec<&mut Tab> = self.tabs.iter_mut().chain(hidden).collect();
        // Signal them all first so they wind down together.
        for t in tabs.iter().filter_map(|t| t.task.as_ref()) {
            t.signal();
        }
        for tab in tabs {
            if tab.task.take().is_some() {
                let detail = "stopped when Mainframe quit".to_string();
                tab.output.push(Line::from(Span::styled(
                    format!("── ✘ {}: {detail}", tab.label),
                    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                )));
                tab.last_run = Some(LastRun { label: tab.label.clone(), ok: false, detail });
            }
        }
        self.save_current();
        for (root, ws) in &self.workspaces {
            ws.save(root);
        }
    }

    /// Tasks running in the project at `root`, shown or not.
    pub fn running_in(&self, root: &Path) -> usize {
        if self.workspace.as_deref() == Some(root) {
            return self.tabs.iter().filter(|t| t.running()).count();
        }
        self.workspaces.get(root).map_or(0, Workspace::running)
    }

    /// Tasks running in projects other than the shown one.
    pub fn running_elsewhere(&self) -> usize {
        self.workspaces.values().map(Workspace::running).sum()
    }
}

// ── on disk ────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    tab: usize,
    show_output: bool,
    zoom_output: bool,
    tabs: Vec<SavedTab>,
}

#[derive(Serialize, Deserialize)]
struct SavedTab {
    label: String,
    /// Where it ran: relative `file:line`s in the output start here.
    cwd: PathBuf,
    /// What ran, so `@` reruns it.
    cmd: Option<String>,
    last_run: Option<(bool, String)>,
    scroll: usize,
    /// With SGR color codes, as `ansi::parse_line` reads them.
    lines: Vec<String>,
}

fn file_for(root: &Path) -> PathBuf {
    config::state_dir().join("output").join(format!("{}.json", config::path_key(root)))
}

/// Writes the tabs, or removes the file when nothing ran. Failing only costs the history.
fn save(root: &Path, tabs: &[Tab], tab: usize, show_output: bool, zoom_output: bool) {
    let path = file_for(root);
    if !used(tabs) {
        let _ = fs::remove_file(&path);
        return;
    }
    let saved = Saved {
        version: VERSION,
        tab,
        show_output,
        zoom_output,
        tabs: tabs
            .iter()
            .map(|t| {
                let skip = t.output.lines.len().saturating_sub(SAVED_LINES);
                SavedTab {
                    label: t.label.clone(),
                    cwd: t.plan.as_ref().map_or_else(|| t.output.cwd().to_path_buf(), |p| p.cwd.clone()),
                    cmd: t.plan.as_ref().map(|p| p.cmd.clone()),
                    last_run: t.last_run.as_ref().map(|l| (l.ok, l.detail.clone())),
                    scroll: t.output.scroll,
                    lines: t.output.lines[skip..].iter().map(ansi::encode_line).collect(),
                }
            })
            .collect(),
    };
    let _ = write_atomic(&path, &serde_json::to_vec(&saved).unwrap_or_default());
}

/// Through a temporary file, so a crash mid-write never leaves half a file behind.
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
}

fn load(root: &Path) -> Option<Workspace> {
    let saved: Saved = serde_json::from_slice(&fs::read(file_for(root)).ok()?).ok()?;
    if saved.version != VERSION || saved.tabs.is_empty() {
        return None;
    }
    let tabs: Vec<Tab> = saved
        .tabs
        .into_iter()
        .map(|t| {
            let mut output = Output::new(&t.cwd);
            for line in &t.lines {
                output.push(ansi::parse_line(line));
            }
            output.scroll = t.scroll.min(output.lines.len());
            Tab {
                last_run: t.last_run.map(|(ok, detail)| LastRun { label: t.label.clone(), ok, detail }),
                plan: t.cmd.map(|cmd| Plan { cmd, cwd: t.cwd }),
                label: t.label,
                output,
                task: None,
                watch: None,
            }
        })
        .collect();
    Some(Workspace {
        tab: saved.tab.min(tabs.len() - 1),
        tabs,
        show_output: saved.show_output,
        zoom_output: saved.zoom_output,
    })
}
