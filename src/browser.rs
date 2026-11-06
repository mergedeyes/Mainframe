use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use ratatui::widgets::ListState;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_exec: bool,
    pub is_symlink: bool,
    pub size: u64,
}

pub struct Browser {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    /// Indices into `entries` that pass the filter.
    pub visible: Vec<usize>,
    pub state: ListState,
    pub filter: String,
    pub show_hidden: bool,
    /// Dotfiles skipped by the last load (when `show_hidden` is off).
    pub hidden_count: usize,
    pub error: Option<String>,
    /// Entries marked with Space, kept across directories.
    pub marked: BTreeSet<PathBuf>,
}

impl Browser {
    pub fn new(cwd: PathBuf, show_hidden: bool) -> Self {
        let mut b = Self {
            cwd,
            entries: Vec::new(),
            visible: Vec::new(),
            state: ListState::default(),
            filter: String::new(),
            show_hidden,
            hidden_count: 0,
            error: None,
            marked: BTreeSet::new(),
        };
        b.reload();
        b
    }

    /// Re-reads the directory, keeping the selection on the same name if possible.
    pub fn reload(&mut self) {
        let keep = self.selected().map(|e| e.name.clone());
        self.load(keep.as_deref());
    }

    fn load(&mut self, select: Option<&str>) {
        self.entries.clear();
        self.hidden_count = 0;
        self.error = None;
        match fs::read_dir(&self.cwd) {
            Ok(rd) => {
                for de in rd.flatten() {
                    let name = de.file_name().to_string_lossy().into_owned();
                    if !self.show_hidden && name.starts_with('.') {
                        self.hidden_count += 1;
                        continue;
                    }
                    let path = de.path();
                    let is_symlink = de.file_type().is_ok_and(|t| t.is_symlink());
                    // Follow symlinks for dir/exec detection.
                    let meta = fs::metadata(&path).or_else(|_| de.metadata());
                    let (is_dir, is_exec, size) = match meta {
                        Ok(m) => (m.is_dir(), m.is_file() && m.permissions().mode() & 0o111 != 0, m.len()),
                        Err(_) => (false, false, 0),
                    };
                    self.entries.push(Entry { name, path, is_dir, is_exec, is_symlink, size });
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.apply_filter();
        if let Some(name) = select {
            self.select_name(name);
        }
    }

    pub fn apply_filter(&mut self) {
        let needle = self.filter.to_lowercase();
        self.visible = (0..self.entries.len())
            .filter(|&i| needle.is_empty() || self.entries[i].name.to_lowercase().contains(&needle))
            .collect();
        let sel = self.state.selected().unwrap_or(0);
        self.state.select(if self.visible.is_empty() {
            None
        } else {
            Some(sel.min(self.visible.len() - 1))
        });
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.state
            .selected()
            .and_then(|i| self.visible.get(i))
            .map(|&i| &self.entries[i])
    }

    /// Space: marks or unmarks the selection and moves on.
    pub fn toggle_mark(&mut self) {
        let Some(path) = self.selected().map(|e| e.path.clone()) else { return };
        if !self.marked.remove(&path) {
            self.marked.insert(path);
        }
        self.move_by(1);
    }

    pub fn select_name(&mut self, name: &str) {
        if let Some(pos) = self.visible.iter().position(|&i| self.entries[i].name == name) {
            self.state.select(Some(pos));
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        let max = self.visible.len() as isize - 1;
        self.state.select(Some((cur + delta).clamp(0, max) as usize));
    }

    pub fn select_first(&mut self) {
        if !self.visible.is_empty() {
            self.state.select(Some(0));
        }
    }

    pub fn select_last(&mut self) {
        if !self.visible.is_empty() {
            self.state.select(Some(self.visible.len() - 1));
        }
    }

    pub fn cd(&mut self, dir: PathBuf) {
        self.cd_select(dir, None);
    }

    fn cd_select(&mut self, dir: PathBuf, select: Option<&str>) {
        self.cwd = dir;
        self.filter.clear();
        self.state = ListState::default();
        self.load(select);
        if self.state.selected().is_none() {
            self.select_first();
        }
    }

    pub fn parent(&mut self) {
        let Some(parent) = self.cwd.parent().map(Path::to_path_buf) else { return };
        let came_from: Option<OsString> = self.cwd.file_name().map(|n| n.to_os_string());
        let name = came_from.map(|n| n.to_string_lossy().into_owned());
        self.cd_select(parent, name.as_deref());
    }
}

pub enum Preview {
    Text(Vec<String>),
    /// `git diff` of a modified file.
    Diff(Vec<String>),
    Binary(u64),
    Dir(Vec<(String, bool)>),
    Error(String),
}

const PREVIEW_BYTES: usize = 64 * 1024;

pub fn preview(entry: &Entry, show_hidden: bool) -> Preview {
    if entry.is_dir {
        return match fs::read_dir(&entry.path) {
            Ok(rd) => {
                let mut items: Vec<(String, bool)> = rd
                    .flatten()
                    .map(|de| (de.file_name().to_string_lossy().into_owned(), de.path().is_dir()))
                    .filter(|(n, _)| show_hidden || !n.starts_with('.'))
                    .collect();
                items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
                items.truncate(500);
                Preview::Dir(items)
            }
            Err(e) => Preview::Error(e.to_string()),
        };
    }
    let mut buf = Vec::with_capacity(PREVIEW_BYTES.min(entry.size as usize));
    match fs::File::open(&entry.path).and_then(|f| f.take(PREVIEW_BYTES as u64).read_to_end(&mut buf)) {
        Ok(_) if looks_binary(&buf) => Preview::Binary(entry.size),
        Ok(_) => Preview::Text(
            String::from_utf8_lossy(&buf)
                .lines()
                .map(|l| l.replace('\t', "    "))
                .collect(),
        ),
        Err(e) => Preview::Error(e.to_string()),
    }
}

pub fn is_binary(path: &Path) -> bool {
    let mut buf = [0u8; 8192];
    fs::File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .is_ok_and(|n| looks_binary(&buf[..n]))
}

fn looks_binary(buf: &[u8]) -> bool {
    buf.iter().take(8192).any(|&b| b == 0)
}

pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n}B") } else { format!("{v:.1}{}", UNITS[u]) }
}
