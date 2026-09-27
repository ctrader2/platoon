//! Plugins in the app: the settings screen's Plugins tab (every plugin, installed or in the catalog, one line each;
//! `d` removes one, enter opens it) and the plugin screen it opens (`Mode::Plugins`, full screen: the list and the
//! selected one's details, install, update, remove, `f`), which goes back to the settings on the same plugin. The
//! catalog is fetched with git on a thread the first time the settings open in a run, and again with `f`; installing
//! and updating run on a thread with a line in the footer. The work itself is in `plugins.rs`.
//!
//! A plugin with skills, squads or agents (instructions Claude follows with the user's permissions) is not installed
//! from a plain yes/no: `Mode::PluginReview` lists each one with what it may do, `enter` reads one in full (Markdown,
//! `mdview.rs`), and `y` installs. An update whose instructions changed is shown again the same way, the changed ones
//! marked (`pluginreview.rs`).

use super::*;
use crate::editor_install::{Dirs, Part, Progress};
use crate::editorplugin;
use crate::pluginreview::{self, Item};
use crate::plugins::{self, Catalog, Installed, Manifest, Origin, Source, Store};
use std::collections::BTreeMap;
use std::sync::mpsc::TryRecvError;

/// Where a plugin stands, as the plugin screen shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlugState {
    /// In the catalog, not installed.
    Available,
    Installed,
    /// Installed, and the catalog has other files for it now.
    Update,
    /// Installed from a folder on this computer.
    FromFolder,
    /// In the catalog, but it can't be installed here: why.
    Unavailable(String),
    /// Installed from the catalog, which doesn't have it any more.
    NotInCatalog,
}

impl PlugState {
    pub fn words(&self) -> String {
        match self {
            PlugState::Available => "Not installed".into(),
            PlugState::Installed => "Installed".into(),
            PlugState::Update => "Installed · update available".into(),
            PlugState::FromFolder => "Installed from a folder".into(),
            PlugState::Unavailable(_) => "Can't be installed here".into(),
            PlugState::NotInCatalog => "Installed · no longer in the catalog".into(),
        }
    }

    /// The same, short, for the one line a plugin has in the settings' list.
    pub fn short_words(&self) -> &'static str {
        match self {
            PlugState::Available => "Available",
            PlugState::Installed => "Installed",
            PlugState::Update => "Update ready",
            PlugState::FromFolder => "Installed from a folder",
            PlugState::Unavailable(_) => "Can't be installed here",
            PlugState::NotInCatalog => "Installed, not in the catalog",
        }
    }

    pub fn installed(&self) -> bool {
        !matches!(self, PlugState::Available | PlugState::Unavailable(_))
    }
}

/// One row of the plugin list (the settings' and the plugin screen's): a plugin of the catalog, or one installed that the catalog doesn't have.
#[derive(Clone, Debug)]
pub struct PlugRow {
    /// The catalog's, or the installed one's when the catalog doesn't have it.
    pub manifest: Manifest,
    pub state: PlugState,
    pub installed: Option<Installed>,
    /// Its folder in the fetched catalog.
    pub entry: Option<PathBuf>,
    /// What installing it downloads besides (the code editor), in bytes.
    pub download_bytes: Option<u64>,
}

/// What a plugin with instructions adds, open to be read before it is installed (or updated).
pub struct Review {
    pub id: String,
    pub name: String,
    /// The versions from and to, for an update.
    pub update: Option<(String, String)>,
    pub items: Vec<Item>,
    /// What the version installed has that this one doesn't: "the agent change-reader".
    pub gone: Vec<String>,
    pub selected: usize,
    /// The item read in full, when one is.
    pub reading: Option<usize>,
    /// The first line in view, of the list and of the item being read; and how many lines the last frame showed.
    pub top: Cell<usize>,
    pub read_top: Cell<usize>,
    pub rows: Cell<usize>,
}

enum JobMsg {
    /// The code editor's downloads, as they go.
    Progress(Progress),
    Done(Result<String, String>),
}

pub struct PluginsState {
    pub store: Store,
    pub source: Source,
    pub catalog: Option<Catalog>,
    pub installed: Vec<Installed>,
    pub selected: usize,
    fetching: Option<Receiver<Result<Catalog, String>>>,
    /// Why the last fetch failed (the cached copy, if any, is still shown).
    pub fetch_error: Option<String>,
    /// The catalog has been asked for in this run (it is fetched once, when the settings first open).
    fetch_tried: bool,
    /// The full-screen plugin screen is open (`Mode::Plugins`, or a question or the review over it).
    pub screen: bool,
    job: Option<Receiver<JobMsg>>,
    /// What the job is doing, for the footer: "Installing Example themes…".
    pub job_label: String,
    /// The remove being asked about was asked from the settings screen, which the answer goes back to.
    from_settings: bool,
    /// The code editor's downloads, as they go (installing the editor plugin).
    progress: BTreeMap<Part, Progress>,
    /// What the installed code editor plugin downloads, and whether all of it is there (`editor_facts`).
    pub editor_bytes: Option<u64>,
    pub editor_complete: bool,
    /// What a plugin with instructions adds, being read before it is installed (`Mode::PluginReview`).
    pub review: Option<Review>,
}

impl PluginsState {
    pub fn new(store: Store, source: Source) -> Self {
        let catalog = plugins::cached(&store, &source);
        let installed = store.installed();
        Self {
            store,
            source,
            catalog,
            installed,
            selected: 0,
            fetching: None,
            fetch_error: None,
            fetch_tried: false,
            screen: false,
            job: None,
            job_label: String::new(),
            from_settings: false,
            progress: BTreeMap::new(),
            editor_bytes: None,
            editor_complete: false,
            review: None,
        }
    }

    /// Look again at the installed code editor plugin: what it downloads, whether all of it is there.
    pub fn editor_facts(&mut self, dirs: &Dirs) {
        let installed = editorplugin::installed(&self.store).is_some();
        self.editor_bytes = installed.then(|| editorplugin::download_bytes(&self.store));
        self.editor_complete = installed && editorplugin::complete(&self.store, dirs);
    }

    /// Where platoon keeps plugins: the real data folder, or in a unit test one beside the test's config (never the
    /// user's), with a catalog source that isn't there until a test gives one.
    pub fn for_config(config: &Path) -> Self {
        #[cfg(test)]
        {
            let store = plugins::store_for(config);
            let base = store.dir.parent().and_then(Path::parent).unwrap_or(config).to_path_buf();
            Self::new(store, Source { url: base.join("no-plugin-source").to_string_lossy().into_owned(), rf: "main".into() })
        }
        #[cfg(not(test))]
        {
            let _ = config;
            Self::new(Store::from_env(), Source::from_env())
        }
    }

    pub fn fetching(&self) -> bool {
        self.fetching.is_some()
    }

    pub fn working(&self) -> bool {
        self.job.is_some()
    }

    /// The catalog's plugins in its order, then the installed ones it doesn't have.
    pub fn rows(&self) -> Vec<PlugRow> {
        let mut out = Vec::new();
        if let Some(c) = &self.catalog {
            for e in &c.entries {
                let installed = self.installed.iter().find(|p| p.id() == e.manifest.id).cloned();
                let state = match &installed {
                    Some(p) if matches!(p.origin, Origin::Folder { .. }) => PlugState::FromFolder,
                    Some(p) if plugins::update_available(p, c) => PlugState::Update,
                    Some(_) => PlugState::Installed,
                    None if !e.problems.is_empty() => PlugState::Unavailable(e.problems.join("; ")),
                    None => PlugState::Available,
                };
                out.push(PlugRow { manifest: e.manifest.clone(), state, installed, entry: Some(e.dir.clone()), download_bytes: e.download_bytes });
            }
        }
        for p in &self.installed {
            if out.iter().any(|r| r.manifest.id == p.id()) {
                continue;
            }
            let state = match p.origin {
                Origin::Folder { .. } => PlugState::FromFolder,
                _ if self.catalog.is_some() => PlugState::NotInCatalog,
                _ => PlugState::Installed,
            };
            let download_bytes = if p.manifest.editor.is_some() { self.editor_bytes } else { None };
            out.push(PlugRow { manifest: p.manifest.clone(), state, installed: Some(p.clone()), entry: None, download_bytes });
        }
        out
    }

    pub fn selected_row(&self) -> Option<PlugRow> {
        let rows = self.rows();
        let i = self.selected.min(rows.len().saturating_sub(1));
        rows.into_iter().nth(i)
    }

    /// Where the plugin `id` is in `rows()`.
    pub fn position(&self, id: &str) -> Option<usize> {
        self.rows().iter().position(|r| r.manifest.id == id)
    }

    /// The id of the plugin at `i` of `rows()`.
    fn id_at(&self, i: usize) -> Option<String> {
        self.rows().into_iter().nth(i).map(|r| r.manifest.id)
    }
}

impl App {
    /// The full-screen plugin screen is showing (on its own, or under a question or the review).
    pub fn plugin_screen_showing(&self) -> bool {
        self.pl.screen
    }

    /// The settings' Plugins tab is showing.
    pub fn plugin_settings_showing(&self) -> bool {
        matches!(self.mode, Mode::Settings) && self.settings_tab == crate::settings::TAB_PLUGINS
    }

    /// Where a question or the review asked from the plugin screen goes back to: the plugin screen, if it is open.
    pub(super) fn plugins_base(&self) -> Mode {
        if self.pl.screen { Mode::Plugins } else { Mode::List }
    }

    /// Enter on a plugin of the settings' Plugins tab: the plugin screen, full screen, on that plugin.
    pub(super) fn open_plugin_screen(&mut self) {
        let rows = self.pl.rows();
        if rows.is_empty() {
            self.notify(if self.pl.fetching() { "The catalog is still being fetched." } else { "No plugins to show yet." });
            return;
        }
        self.pl.selected = self.setting.min(rows.len() - 1);
        self.pl.screen = true;
        self.mode = Mode::Plugins;
        self.notice = None;
        self.plugins_shown();
    }

    /// Back from the plugin screen: the settings, on their Plugins tab, on the plugin that was selected.
    pub(super) fn close_plugin_screen(&mut self) {
        self.pl.screen = false;
        self.pl.review = None;
        self.refresh_settings();
        self.settings_tab = crate::settings::TAB_PLUGINS;
        self.setting = self.pl.selected.min(self.pl.rows().len().saturating_sub(1));
        self.setting_top.set(0);
        self.notice = None;
        self.mode = Mode::Settings;
    }

    /// The keys of the plugin screen. It is not a top-level list: the quit key goes back, like esc and the back key
    /// (which arrives here as esc), and `s` and the tab numbers do nothing.
    pub(super) fn plugin_screen_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.keys.get().quit.matches(&key) || key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('c')) {
            return self.close_plugin_screen();
        }
        self.plugin_list_key(key);
    }

    /// Something of the plugins is running on a thread (the event loop refreshes more often meanwhile).
    pub fn plugins_busy(&self) -> bool {
        self.pl.fetching() || self.pl.working()
    }

    /// Called when the settings (or the plugin screen) open: the catalog is fetched the first time in a run.
    pub(super) fn plugins_shown(&mut self) {
        if !self.pl.fetch_tried {
            self.fetch_catalog();
        }
    }

    /// Fetch the catalog on a thread (`f`, and the first time the settings open).
    pub(super) fn fetch_catalog(&mut self) {
        if self.pl.fetching() {
            return;
        }
        self.pl.fetch_tried = true;
        let (store, source) = (self.pl.store.clone(), self.pl.source.clone());
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(plugins::fetch(&store, &source));
        });
        self.pl.fetching = Some(rx);
    }

    /// Called on every refresh: a fetch or an install that finished.
    pub(super) fn poll_plugins(&mut self) {
        if let Some(rx) = &self.pl.fetching {
            match rx.try_recv() {
                Ok(Ok(c)) => {
                    // The rows may come in another order now: the same plugins stay selected.
                    let (on_screen, in_settings) = (self.pl.id_at(self.pl.selected), self.pl.id_at(self.setting));
                    self.pl.catalog = Some(c);
                    self.pl.fetch_error = None;
                    self.pl.fetching = None;
                    self.keep_plugin_selection(on_screen, in_settings);
                }
                Ok(Err(e)) => {
                    if self.plugin_screen_showing() {
                        self.notify(e.clone());
                    }
                    self.pl.fetch_error = Some(e);
                    self.pl.fetching = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.pl.fetch_error = Some("platoon couldn't reach its plugin catalog: the fetch stopped half way".into());
                    self.pl.fetching = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.pl.job {
            let last = loop {
                match rx.try_recv() {
                    Ok(JobMsg::Progress(p)) => {
                        self.pl.progress.insert(p.part, p);
                    }
                    other => break other,
                }
            };
            match last {
                Ok(JobMsg::Progress(_)) => {}
                Ok(JobMsg::Done(r)) => {
                    self.pl.job = None;
                    self.plugins_changed();
                    match r {
                        Ok(msg) | Err(msg) => self.notify(msg),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.pl.job = None;
                    self.plugins_changed();
                    self.notify("The plugin didn't finish installing; nothing was changed.");
                }
                Err(TryRecvError::Empty) => {
                    let parts: Vec<String> = self
                        .pl
                        .progress
                        .values()
                        .map(|p| if p.done + p.failed >= p.total && p.failed == 0 { format!("{} ✓", p.part.label()) } else { format!("{} {}/{}", p.part.label(), p.done, p.total) })
                        .collect();
                    let line = if parts.is_empty() { self.pl.job_label.clone() } else { format!("{} {}", self.pl.job_label.trim_end_matches('…'), parts.join("  ")) };
                    self.notify(line);
                }
            }
        }
    }

    /// Read what is installed again, and with it the themes (a theme in use whose plugin went gives way to the
    /// default; one whose plugin came back is used again) and the code editor's state.
    pub(super) fn plugins_changed(&mut self) {
        let (on_screen, in_settings) = (self.pl.id_at(self.pl.selected), self.pl.id_at(self.setting));
        self.pl.installed = self.pl.store.installed();
        // A plugin's squads and agents come and go with it.
        self.poll_squad_files();
        let dirs = self.ed.dirs.clone();
        self.pl.editor_facts(&dirs);
        self.keep_plugin_selection(on_screen, in_settings);
        self.themes.refresh();
        self.sync_theme();
    }

    /// After the rows changed: the plugins that were selected (on the plugin screen, and in the settings' Plugins tab
    /// when it is the one showing) stay selected where they still are; otherwise the selection stays in range.
    fn keep_plugin_selection(&mut self, on_screen: Option<String>, in_settings: Option<String>) {
        let last = self.pl.rows().len().saturating_sub(1);
        self.pl.selected = on_screen.and_then(|id| self.pl.position(&id)).unwrap_or(self.pl.selected).min(last);
        if self.settings_tab == crate::settings::TAB_PLUGINS {
            self.setting = in_settings.and_then(|id| self.pl.position(&id)).unwrap_or(self.setting).min(last);
        }
    }

    /// The wheel on the plugin screen: up and down the list.
    pub(super) fn wheel_plugins(&mut self, up: bool) {
        self.move_plugin(if up { -1 } else { 1 });
    }

    fn move_plugin(&mut self, delta: i32) {
        let last = self.pl.rows().len().saturating_sub(1) as i32;
        self.pl.selected = (self.pl.selected as i32 + delta).clamp(0, last) as usize;
    }

    /// A click on a row of the plugin screen: select it (clicking the selected one again is enter).
    pub(super) fn click_plugin(&mut self, i: usize) {
        if i == self.pl.selected {
            self.plugin_enter();
        } else {
            self.pl.selected = i;
        }
    }

    pub(super) fn plugin_list_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.move_plugin(-1),
            KeyCode::Down => self.move_plugin(1),
            KeyCode::PageUp => self.move_plugin(-5),
            KeyCode::PageDown => self.move_plugin(5),
            KeyCode::Char('g') | KeyCode::Home => self.pl.selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.move_plugin(i32::MAX / 2),
            KeyCode::Enter | KeyCode::Char('i') => self.plugin_enter(),
            KeyCode::Char('u') => self.ask_update_plugin(),
            KeyCode::Char('d') => self.ask_remove_plugin(false),
            KeyCode::Char('f') => {
                self.fetch_catalog();
                self.notify("Checking the catalog for plugins…");
            }
            _ => {}
        }
    }

    /// Enter on a plugin: install it, update it, or say why not.
    fn plugin_enter(&mut self) {
        let Some(row) = self.pl.selected_row() else {
            self.notify(if self.pl.fetching() { "The catalog is still being fetched." } else { "No plugins to show. f checks the catalog again." });
            return;
        };
        match &row.state {
            PlugState::Available if row.manifest.has_instructions() => {
                if self.pl.working() {
                    self.notify("Another plugin is being installed; wait for it to finish.");
                    return;
                }
                self.open_review(&row, None);
            }
            PlugState::Available => {
                if self.pl.working() {
                    self.notify("Another plugin is being installed; wait for it to finish.");
                    return;
                }
                self.mode = Mode::Confirm(Confirm::InstallPlugin {
                    id: row.manifest.id.clone(),
                    name: row.manifest.name.clone(),
                    adds: row.manifest.adds_words(),
                    update: None,
                    download: row.download_bytes,
                });
            }
            PlugState::Update => self.ask_update_plugin(),
            PlugState::Unavailable(why) => self.notify(format!("{} can't be installed here: {why}", row.manifest.name)),
            // The code editor plugin whose downloads didn't all finish: enter finishes them.
            _ if row.manifest.editor.is_some() && !self.pl.editor_complete => {
                if self.pl.working() || self.editor_busy() {
                    self.notify("The code editor is being installed; wait for it to finish.");
                } else {
                    self.ask_install_editor();
                }
            }
            _ => self.notify(format!("{} is installed. d removes it.", row.manifest.name)),
        }
    }

    fn ask_update_plugin(&mut self) {
        let Some(row) = self.pl.selected_row() else { return };
        if row.state != PlugState::Update {
            let msg = match row.state {
                PlugState::FromFolder => format!("{} was installed from a folder: `platoon plugin add` that folder again to update it.", row.manifest.name),
                _ if row.state.installed() => format!("{} is up to date.", row.manifest.name),
                _ => format!("{} isn't installed.", row.manifest.name),
            };
            self.notify(msg);
            return;
        }
        if self.pl.working() {
            self.notify("Another plugin is being installed; wait for it to finish.");
            return;
        }
        let was = row.installed.as_ref().map(|p| p.manifest.version.clone()).unwrap_or_default();
        // Instructions Claude follows that the update changes are shown again first.
        if row.manifest.has_instructions() || row.installed.as_ref().is_some_and(|p| p.manifest.has_instructions()) {
            let old = self.pl.store.plugin_dir(&row.manifest.id);
            if let Some(dir) = row.entry.as_ref() {
                match pluginreview::review(dir, Some(&old)) {
                    Ok((_, r)) if r.changed() => return self.open_review(&row, Some((was, row.manifest.version.clone()))),
                    Ok(_) => {}
                    Err(e) => return self.notify(e),
                }
            }
        }
        self.mode = Mode::Confirm(Confirm::InstallPlugin {
            id: row.manifest.id.clone(),
            name: row.manifest.name.clone(),
            adds: row.manifest.adds_words(),
            update: Some((was, row.manifest.version.clone())),
            download: row.download_bytes,
        });
    }

    /// `d`: take the selected plugin off (on the plugin screen, or in the settings' Plugins tab).
    pub(super) fn ask_remove_plugin(&mut self, from_settings: bool) {
        let row = if from_settings { self.pl.rows().into_iter().nth(self.setting) } else { self.pl.selected_row() };
        let Some(row) = row else { return };
        let Some(p) = row.installed.clone() else {
            self.notify(format!("{} isn't installed.", row.manifest.name));
            return;
        };
        if self.pl.working() {
            self.notify("A plugin is being installed; wait for it to finish.");
            return;
        }
        let in_use = self.themes.plugin_of(&self.theme.id).is_some() && self.theme.id.starts_with(&format!("{}{}/", plugins::THEME_PREFIX, p.id()));
        self.pl.from_settings = from_settings;
        if p.manifest.editor.is_some() && (self.editor_busy() || self.ed_open_now()) {
            self.notify("The code editor is being installed or is open; try again when it has finished.");
            return;
        }
        let editor = p.manifest.editor.is_some();
        let m = &p.manifest;
        let mut extra: Vec<String> = Vec::new();
        if !m.files(crate::skillplugin::KIND).is_empty() {
            extra.push("Its skills leave Claude Code's skills folder (one you changed is kept).".into());
        }
        if !m.files(crate::squadplugin::SQUADS).is_empty() {
            extra.push("Tickets its squads already have keep working as they started.".into());
        }
        if let Some(w) = crate::squadplugin::removal_warning(&crate::squadplugin::users_of_plugin(&self.config, p.id())) {
            extra.push(w);
        }
        self.mode = Mode::Confirm(Confirm::RemovePlugin { id: p.id().to_string(), name: m.name.clone(), adds: m.adds_words(), in_use, editor, extra: extra.join(" ") });
    }

    // ---- reading what a plugin adds before it is installed -------------------------------------------------------

    /// Show what the plugin of `row` adds that Claude follows (`update`: the versions, when it replaces the one
    /// installed, whose items are compared with it).
    fn open_review(&mut self, row: &PlugRow, update: Option<(String, String)>) {
        let Some(dir) = row.entry.clone() else { return };
        let old = update.as_ref().map(|_| self.pl.store.plugin_dir(&row.manifest.id));
        match pluginreview::review(&dir, old.as_deref()) {
            Ok((m, r)) => {
                self.pl.review = Some(Review {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    update,
                    items: r.items,
                    gone: r.gone,
                    selected: 0,
                    reading: None,
                    top: Cell::new(0),
                    read_top: Cell::new(0),
                    rows: Cell::new(10),
                });
                self.mode = Mode::PluginReview;
            }
            Err(e) => self.notify(e),
        }
    }

    /// `y` on the review: install (or update) it.
    pub(super) fn accept_review(&mut self) {
        let Some(r) = self.pl.review.take() else {
            self.mode = self.plugins_base();
            return;
        };
        self.install_plugin(&r.id);
    }

    /// `n` or esc on the review: nothing is installed.
    pub(super) fn close_review(&mut self) {
        self.pl.review = None;
        self.mode = self.plugins_base();
    }

    pub(super) fn review_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(r) = self.pl.review.as_mut() else {
            self.mode = self.plugins_base();
            return;
        };
        if r.reading.is_some() {
            let page = r.rows.get().saturating_sub(1).max(1) as i64;
            let delta: i64 = match key.code {
                KeyCode::Esc | KeyCode::Left | KeyCode::Backspace | KeyCode::Enter | KeyCode::Char('q') => {
                    r.reading = None;
                    return;
                }
                KeyCode::Char('c') if ctrl => {
                    r.reading = None;
                    return;
                }
                KeyCode::Up | KeyCode::Char('k') => -1,
                KeyCode::Down | KeyCode::Char('j') => 1,
                KeyCode::PageUp => -page,
                KeyCode::PageDown | KeyCode::Char(' ') => page,
                KeyCode::Home | KeyCode::Char('g') => i64::MIN / 2,
                KeyCode::End | KeyCode::Char('G') => i64::MAX / 2,
                _ => 0,
            };
            // Clamped when it is drawn, to the text's length at the width it has.
            r.read_top.set((r.read_top.get() as i64).saturating_add(delta).max(0) as usize);
            return;
        }
        match key.code {
            KeyCode::Up => r.selected = r.selected.saturating_sub(1),
            KeyCode::Down => r.selected = (r.selected + 1).min(r.items.len().saturating_sub(1)),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('r') => {
                if !r.items.is_empty() {
                    r.reading = Some(r.selected);
                    r.read_top.set(0);
                }
            }
            KeyCode::Char('y') => self.act(Action::Confirm),
            KeyCode::Char('n') | KeyCode::Esc => self.act(Action::Cancel),
            KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
            _ => {}
        }
    }

    pub(super) fn wheel_review(&mut self, up: bool) {
        let Some(r) = self.pl.review.as_mut() else { return };
        if r.reading.is_some() {
            r.read_top.set(if up { r.read_top.get().saturating_sub(3) } else { r.read_top.get() + 3 });
        } else if up {
            r.selected = r.selected.saturating_sub(1);
        } else {
            r.selected = (r.selected + 1).min(r.items.len().saturating_sub(1));
        }
    }

    /// Yes to installing (or updating) a plugin from the catalog: on a thread, with a line in the footer.
    pub(super) fn install_plugin(&mut self, id: &str) {
        self.mode = self.plugins_base();
        self.pl.review = None;
        let (Some(c), Some(row)) = (self.pl.catalog.clone(), self.pl.rows().into_iter().find(|r| r.manifest.id == id)) else { return };
        let Some(dir) = row.entry.clone() else { return };
        let store = self.pl.store.clone();
        let origin = Origin::Catalog { source: c.source.clone(), rf: c.rf.clone(), commit: c.commit.clone() };
        let name = row.manifest.name.clone();
        let updating = row.state == PlugState::Update;
        let dirs = self.ed.dirs.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let r = store.install_noting(&dir, origin).map_err(|e| format!("Not installed: {e}")).and_then(|(o, notes)| {
                let m = &o.plugin().manifest;
                let mut said = match o {
                    plugins::Outcome::Replaced { ref was, .. } if *was != m.version => format!("{} is updated from {was} to {}.", m.name, m.version),
                    plugins::Outcome::Replaced { .. } => format!("{} {} is installed from the catalog.", m.name, m.version),
                    _ if m.editor.is_some() => format!("{} is installed.", m.name),
                    _ => format!("{} is installed: {} added. {}", m.name, m.adds_words(), plugins::next_words(m)),
                };
                for n in notes {
                    said.push(' ');
                    said.push_str(&n);
                }
                if m.editor.is_none() {
                    return Ok(said.trim_end().to_string());
                }
                // The code editor: then what its table has that the editor folder doesn't.
                let progress = tx.clone();
                let report = move |p| {
                    let _ = progress.send(JobMsg::Progress(p));
                };
                match editorplugin::install_downloads(&store, &dirs, &report) {
                    Ok(_) => Ok(format!("{said} v on a session or a ticket opens it.")),
                    Err(e) => Err(format!("{said} Its downloads didn't finish: {e} Enter on it in the settings' Plugins tab tries again.")),
                }
            });
            let _ = tx.send(JobMsg::Done(r));
        });
        self.pl.progress.clear();
        self.pl.job = Some(rx);
        self.pl.job_label = if updating { format!("Updating {name}…") } else { format!("Installing {name}…") };
        let line = self.pl.job_label.clone();
        self.notify(line);
    }

    /// Yes to removing a plugin: quick (a few small files; the code editor's folder a moment longer), so done at once.
    pub(super) fn remove_plugin(&mut self, id: &str) {
        self.mode = if std::mem::take(&mut self.pl.from_settings) { Mode::Settings } else { self.plugins_base() };
        let was_using = self.theme.id.starts_with(&format!("{}{id}/", plugins::THEME_PREFIX));
        if editorplugin::is_editor(&self.pl.store, id) {
            let r = editorplugin::remove(&self.pl.store, &self.ed.dirs, id);
            self.plugins_changed();
            match r {
                Ok((p, removed)) => {
                    let name = p.map_or_else(|| "The code editor".to_string(), |p| p.manifest.name);
                    let size = if removed.editor { format!(" ({})", crate::editor_install::megabytes(removed.bytes)) } else { String::new() };
                    self.notify(format!("Removed {name} and everything it downloaded{size}. v still opens your other editors."));
                }
                Err(e) => self.notify(format!("Not removed: {e}")),
            }
            return;
        }
        let r = self.pl.store.remove_noting(id);
        self.plugins_changed();
        let notes = |n: &[String]| if n.is_empty() { String::new() } else { format!(" {}", n.join(" ")) };
        match r {
            Ok((Some(p), n)) if was_using => self.notify(format!("Removed {}. Its theme was in use, so platoon is back to {}.{}", p.manifest.name, self.theme.name, notes(&n))),
            Ok((Some(p), n)) => self.notify(format!("Removed {}.{}", p.manifest.name, notes(&n))),
            Ok((None, _)) => self.notify("That plugin wasn't installed."),
            Err(e) => self.notify(format!("Not removed: {e}")),
        }
    }

    /// No to removing: back where it was asked.
    pub(super) fn cancel_remove_plugin(&mut self) {
        self.mode = if std::mem::take(&mut self.pl.from_settings) { Mode::Settings } else { self.plugins_base() };
    }

    /// The settings screen's Plugins tab: up and down over every plugin, enter opens the plugin screen on the one
    /// selected, `d` removes an installed one.
    pub(super) fn plugin_settings_key(&mut self, key: KeyEvent) -> bool {
        if self.settings_tab != crate::settings::TAB_PLUGINS {
            return false;
        }
        let last = self.pl.rows().len().saturating_sub(1);
        match key.code {
            KeyCode::Up => self.setting = self.setting.saturating_sub(1).min(last),
            KeyCode::Down => self.setting = (self.setting + 1).min(last),
            KeyCode::PageUp => self.setting = self.setting.saturating_sub(5).min(last),
            KeyCode::PageDown => self.setting = (self.setting + 5).min(last),
            KeyCode::Home | KeyCode::Char('g') => self.setting = 0,
            KeyCode::End | KeyCode::Char('G') => self.setting = last,
            KeyCode::Enter => self.open_plugin_screen(),
            KeyCode::Char('d') | KeyCode::Delete => self.ask_remove_plugin(true),
            KeyCode::Char('f') => self.fetch_catalog(),
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ' | 'h' | 'l' | '-' | '+' | '=' | 'r') => {}
            _ => return false,
        }
        true
    }
}
