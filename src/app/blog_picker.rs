//! `L` in the blog repo: its drafts and posts. New posts, editing (in `post_editor`, or the
//! configured editor), publishing and pushing.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Confirmed, Level, Mode, files::trash_cmd, post_editor::Part};
use crate::{
    blog::{self, Change, Post, State},
    filter_list::{self, FilterList, Match},
    form::{Field, Form, FormKind},
    project::Plan,
};

pub struct BlogPicker {
    pub root: PathBuf,
    pub list: FilterList<Post>,
    /// `/` was pressed: keys go to the filter until Enter or Esc. Otherwise letters are commands.
    pub filtering: bool,
}

impl App {
    /// Opens the picker on the blog, with `select` selected when it's listed.
    pub(super) fn open_blog_picker(&mut self, select: Option<&Path>) {
        let Some(root) = self.blog_root.clone() else { return };
        let mut list = FilterList::new(
            blog::list(&root),
            |p| format!("{} {} {}", p.title, p.slug, p.date.as_deref().unwrap_or_default()),
            Match::Fuzzy { limit: usize::MAX },
        );
        if let Some(path) = select {
            list.select_where(|p| p.path == path);
        }
        self.mode = Mode::Blog(BlogPicker { root, list, filtering: false });
    }

    pub(super) fn key_blog_picker(&mut self, key: KeyEvent) {
        let Mode::Blog(picker) = &mut self.mode else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if picker.filtering {
            match key.code {
                KeyCode::Enter => picker.filtering = false,
                KeyCode::Esc => {
                    picker.list.clear_filter();
                    picker.filtering = false;
                }
                _ => {
                    if picker.list.key(key) == filter_list::Key::Close {
                        self.mode = Mode::Normal;
                    }
                }
            }
            return;
        }
        let len = picker.list.visible.len();
        let state = &mut picker.list.state;
        match key.code {
            KeyCode::Char('c') if ctrl => self.mode = Mode::Normal,
            KeyCode::Esc if !picker.list.filter.is_empty() => picker.list.clear_filter(),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('L') => self.mode = Mode::Normal,
            KeyCode::Char('/') => picker.filtering = true,
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => filter_list::step(state, len, 1),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => filter_list::step(state, len, -1),
            KeyCode::PageDown => filter_list::step(state, len, 10),
            KeyCode::PageUp => filter_list::step(state, len, -10),
            KeyCode::Home | KeyCode::Char('g') => filter_list::step(state, len, -(len as isize)),
            KeyCode::End | KeyCode::Char('G') => filter_list::step(state, len, len as isize),
            KeyCode::Char('a') => self.new_post_form(),
            KeyCode::Enter | KeyCode::Char('e') => self.edit_post(false),
            KeyCode::Char('E') => self.edit_post(true),
            KeyCode::Char('p') => self.confirm_publish(),
            KeyCode::Char('P') => self.push_post(),
            KeyCode::Char('u') => self.unpublish_post(),
            KeyCode::Char('d') | KeyCode::Delete => self.confirm_delete_post(),
            _ => {}
        }
    }

    /// The picker's root and selected post, if any.
    fn picked_post(&self) -> Option<(PathBuf, Post)> {
        let Mode::Blog(picker) = &self.mode else { return None };
        Some((picker.root.clone(), picker.list.selected()?.clone()))
    }

    fn new_post_form(&mut self) {
        let Some(root) = self.blog_root.clone() else { return };
        self.mode = Mode::Form(Form {
            kind: FormKind::BlogPost { root },
            title: " new blog post ".into(),
            fields: vec![
                Field::text("title", "", "required"),
                Field::text("description", "", "optional, one line"),
                Field::choice("save as", &["draft", "post now"], 0, "←/→ · d / p"),
            ],
            step: 0,
            note: None,
            completer: None,
        });
    }

    /// The form's Enter: writes the post and opens it in the editor. False keeps the form open.
    pub(super) fn submit_blog_form(&mut self, form: &Form) -> bool {
        let FormKind::BlogPost { root } = &form.kind else { return false };
        let draft = form.get("save as") == "draft";
        match blog::create(root, form.get("title"), form.get("description"), draft, &blog::today()) {
            Ok(path) => {
                self.set_status(format!("wrote {}", blog::rel(root, &path)), Level::Ok);
                self.open_post_editor(root.clone(), &path, Some(Part::Body));
                true
            }
            Err(e) => {
                self.set_status(e, Level::Err);
                false
            }
        }
    }

    /// The built-in editor on the selected post, or with `external` the configured one.
    fn edit_post(&mut self, external: bool) {
        let Some((root, post)) = self.picked_post() else { return };
        if external {
            self.edit_post_file(&post.path);
        } else {
            self.open_post_editor(root, &post.path, None);
        }
    }

    /// The external editor on a post; the picker comes back afterwards.
    fn edit_post_file(&mut self, path: &Path) {
        self.mode = Mode::Normal;
        if self.edit_path(path, "") {
            self.reopen_blog = Some(path.to_path_buf());
        }
    }

    fn confirm_publish(&mut self) {
        let Some((root, post)) = self.picked_post() else { return };
        if post.state == State::Post {
            return self.set_status(format!("{} is published — P pushes your changes", post.rel()), Level::Info);
        }
        let target = blog::publish_target(&root, &post, &blog::today());
        let target_rel = blog::rel(&root, &target);
        if target.exists() {
            return self.set_status(format!("{target_rel} already exists"), Level::Err);
        }
        self.mode = Mode::Confirm {
            msg: format!("Publish as {target_rel}, commit \"post: {}\" and push? [y/N]", post.title),
            action: Confirmed::BlogPublish(root, Box::new(post)),
        };
    }

    /// After the confirmation: moves the draft into `_posts`, commits and pushes.
    pub(super) fn publish_post(&mut self, root: PathBuf, draft: &Post) {
        match blog::publish(&root, draft, &blog::today()) {
            Ok(path) => self.commit_post(root, &path, format!("post: {}", draft.title)),
            Err(e) => {
                self.set_status(e, Level::Err);
                self.open_blog_picker(Some(&draft.path));
            }
        }
    }

    fn push_post(&mut self) {
        let Some((root, post)) = self.picked_post() else { return };
        if post.state == State::Draft {
            return self.set_status(format!("{} is a draft — p publishes it", post.rel()), Level::Info);
        }
        let msg = match blog::change(&root, &post.path) {
            Change::None => return self.set_status(format!("no changes to {}", post.rel()), Level::Info),
            Change::New => format!("post: {}", post.title),
            Change::Edited => format!("post: update {}", post.title),
        };
        self.mode = Mode::Normal;
        self.commit_post(root, &post.path, msg);
    }

    /// Commits and pushes in the output pane, then follows the GitHub Pages build when it can.
    pub(super) fn commit_post(&mut self, root: PathBuf, path: &Path, msg: String) {
        let mut cmd = blog::commit_cmd(&root, &msg);
        if let Some(follow) = blog::follow_pages_cmd(&root, path) {
            cmd = format!("{cmd} && {follow}");
        }
        self.start_task(msg, Plan { cmd, cwd: root });
    }

    fn unpublish_post(&mut self) {
        let Some((root, post)) = self.picked_post() else { return };
        match blog::unpublish(&root, &post) {
            Ok(path) => {
                self.open_blog_picker(Some(&path));
                let msg = format!("moved back to {} — not committed", blog::rel(&root, &path));
                self.set_status(msg, Level::Ok);
            }
            Err(e) => self.set_status(e, Level::Err),
        }
    }

    fn confirm_delete_post(&mut self) {
        let Some((_, post)) = self.picked_post() else { return };
        let verb = if trash_cmd().is_some() { "Trash" } else { "Permanently delete" };
        self.mode =
            Mode::Confirm { msg: format!("{verb} {}? [y/N]", post.rel()), action: Confirmed::BlogDelete(post.path) };
    }

    pub(super) fn delete_post(&mut self, path: PathBuf) {
        self.delete(std::slice::from_ref(&path));
        self.open_blog_picker(None);
    }
}
