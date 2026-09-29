//! Plugins in the app: the settings screen's Plugins tab (every plugin, installed or in the catalog, one line each, and
//! the selected one's description; `d` removes one, enter opens it) and the plugin screen it opens (`Mode::Plugins`,
//! full screen: the list and the selected one's details, install, update, remove; `1` `2` `3` go straight to a tab),
//! which goes back to the settings on the same plugin. The catalog is built into platoon (`plugins.rs`), read the first
//! time it is needed in a run; installing and updating run on a thread with a line in the footer.
//!
//! When platoon starts, the plugins installed from the catalog that this platoon has a newer version of are updated
//! (`update_with_platoon`): themes and the code editor quietly on a thread (the editor's changed downloads too, with
//! the progress in the footer), said in one line; one with skills, squads or agents is shown in the review and asked
//! about, one at a time, when the list is showing. A no is remembered for that version (`Store::decline_update`).
//!
//! A plugin with skills, squads or agents (instructions Claude follows with the user's permissions) is not installed
//! from a plain yes/no: `Mode::PluginReview` lists each one with what it may do, `enter` reads one in full (Markdown,
//! `mdview.rs`), and `y` installs. An update whose instructions changed is shown again the same way, the changed ones
//! marked (`pluginreview.rs`).

use super::*;
use crate::editor_install::{Dirs, Part, Progress};
use crate::editorplugin;
use crate::pluginreview::{self, Item};
use crate::plugins::{self, Catalog, CatalogFrom, Entry, Installed, Manifest, Origin, Store};
use std::collections::BTreeMap;
use std::sync::mpsc::TryRecvError;

/// Where a plugin stands, as the plugin screen shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlugState {
    /// In the catalog, not installed.
    Available,
    Installed,
    /// Installed, and the catalog has a newer version of it.
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
    /// The catalog's plugin (its files, to show and install).
    pub entry: Option<Entry>,
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
    /// platoon asked about this update when it started (a no is remembered for this version).
    pub at_start: bool,
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
    /// Where the catalog comes from: built into platoon (a test gives a folder).
    pub from: CatalogFrom,
    /// The catalog, once read in this run (`load_catalog`).
    pub catalog: Option<Catalog>,
    pub installed: Vec<Installed>,
    pub selected: usize,
    /// Updates of plugins with instructions this platoon brings, still to be asked about (one at a time).
    pub asks: Vec<plugins::Ask>,
    /// The job running is the quiet update at start: nothing is said until it has something to say.
    quiet_job: bool,
    /// The full-screen plugin screen is open (`Mode::Plugins`, or a question or the review over it).
    pub screen: bool,
    job: Option<Receiver<JobMsg>>,
    /// What the job is doing, for the footer: "Installing Theme pack…".
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
    pub fn new(store: Store, from: CatalogFrom) -> Self {
        let installed = store.installed();
        Self {
            store,
            from,
            catalog: None,
            installed,
            selected: 0,
            asks: Vec::new(),
            quiet_job: false,
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
    /// user's), with an empty catalog until a test gives one (`use_catalog`).
    pub fn for_config(config: &Path) -> Self {
        #[cfg(test)]
        {
            let store = plugins::store_for(config);
            let base = store.dir.parent().and_then(Path::parent).unwrap_or(config).to_path_buf();
            Self::new(store, CatalogFrom::Folder(base.join("no-plugin-catalog")))
        }
        #[cfg(not(test))]
        {
            let _ = config;
            Self::new(Store::from_env(), CatalogFrom::from_env())
        }
    }

    /// Read the catalog, the first time it is needed in a run.
    pub fn ensure_catalog(&mut self) {
        if self.catalog.is_none() {
            self.catalog = Some(plugins::load_catalog(&self.from));
        }
    }

    /// Take the catalog from `from` (a test), read afresh.
    #[cfg(test)]
    pub fn use_catalog(&mut self, from: CatalogFrom) {
        self.from = from;
        self.catalog = Some(plugins::load_catalog(&self.from));
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
                out.push(PlugRow { manifest: e.manifest.clone(), state, installed, entry: Some(e.clone()), download_bytes: e.download_bytes });
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
            self.notify("No plugins to show.");
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
    /// (which arrives here as esc), and `s` does nothing. `1` `2` `3` go straight to Sessions, Projects, Squads (the
    /// plugin screen and the settings close).
    pub(super) fn plugin_screen_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if self.keys.get().quit.matches(&key) || key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('c')) {
            return self.close_plugin_screen();
        }
        if let KeyCode::Char(c @ '1'..='3') = key.code {
            if !ctrl && !alt {
                self.pl.screen = false;
                self.pl.review = None;
                self.notice = None;
                self.mode = Mode::List;
                self.switch_top(c as usize - '1' as usize);
                return;
            }
        }
        self.plugin_list_key(key);
    }

    /// Something of the plugins is running on a thread (the event loop refreshes more often meanwhile).
    pub fn plugins_busy(&self) -> bool {
        self.pl.working()
    }

    /// Called when the settings (or the plugin screen) open: the catalog is read the first time in a run.
    pub(super) fn plugins_shown(&mut self) {
        if self.pl.catalog.is_none() {
            let (on_screen, in_settings) = (self.pl.id_at(self.pl.selected), self.pl.id_at(self.setting));
            self.pl.ensure_catalog();
            self.keep_plugin_selection(on_screen, in_settings);
        }
    }

    /// When platoon starts: the copy of the catalog an older platoon fetched goes, and the plugins installed from the
    /// catalog that this platoon has a newer version of are updated: themes and the code editor on a thread, said in
    /// one line when done (nothing said when nothing was updated); the ones with instructions are asked about, one at
    /// a time, once the list is showing (`poll_plugins`).
    pub(crate) fn update_with_platoon(&mut self) {
        self.pl.store.forget_fetched_catalog();
        // Only a plugin from the catalog can be updated by it: without one, the catalog isn't even read.
        if !plugins::any_from_catalog(&self.pl.store) {
            return;
        }
        self.pl.ensure_catalog();
        let Some(catalog) = self.pl.catalog.clone() else { return };
        let plan = plugins::auto_plan(&self.pl.store, &catalog);
        self.pl.asks = plan.ask.clone();
        if plan.quiet.is_empty() || self.pl.working() {
            return;
        }
        let (store, dirs) = (self.pl.store.clone(), self.ed.dirs.clone());
        let (tx, rx) = channel();
        thread::spawn(move || {
            let (done, failed) = plugins::auto_update(&store, &catalog, &plan);
            let mut said = plugins::updated_words(&done);
            for f in &failed {
                said = format!("{said} {f}.").trim_start().to_string();
            }
            // The code editor's downloads that changed with it, as an update fetches them.
            if done.iter().any(|(_, _, editor)| *editor) {
                let progress = tx.clone();
                let report = move |p| {
                    let _ = progress.send(JobMsg::Progress(p));
                };
                if let Err(e) = editorplugin::install_downloads(&store, &dirs, &report) {
                    said = format!("{said} The code editor's downloads didn't finish: {e} Enter on it in the settings (s, then Plugins) tries again.");
                }
            }
            let _ = tx.send(JobMsg::Done(if failed.is_empty() { Ok(said) } else { Err(said) }));
        });
        self.pl.progress.clear();
        self.pl.job = Some(rx);
        self.pl.quiet_job = true;
        self.pl.job_label = "Updating the code editor…".into();
    }

    /// The next update platoon asks about at start, when nothing else is going on: its review, over the list.
    fn ask_next_update(&mut self) {
        if self.pl.asks.is_empty() || self.pl.working() || !matches!(self.mode, Mode::List) || self.pl.review.is_some() {
            return;
        }
        let ask = self.pl.asks.remove(0);
        let Some(row) = self.pl.rows().into_iter().find(|r| r.manifest.id == ask.id && r.state == PlugState::Update) else { return };
        self.open_review(&row, Some((ask.from, ask.to)));
        if let Some(r) = self.pl.review.as_mut() {
            r.at_start = true;
        }
    }

    /// Called on every refresh: an install that finished, and an update to ask about.
    pub(super) fn poll_plugins(&mut self) {
        self.ask_next_update();
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
                    let quiet = std::mem::take(&mut self.pl.quiet_job);
                    self.plugins_changed();
                    match r {
                        // Nothing updated after all (another platoon did it a moment before): nothing to say.
                        Ok(msg) if msg.is_empty() => {
                            if quiet {
                                self.notice = None;
                            }
                        }
                        Ok(msg) | Err(msg) => self.notify(msg),
                    }
                    self.ask_next_update();
                }
                Err(TryRecvError::Disconnected) => {
                    self.pl.job = None;
                    self.pl.quiet_job = false;
                    self.plugins_changed();
                    self.notify("The plugin didn't finish installing; nothing was changed.");
                }
                Err(TryRecvError::Empty) if self.pl.quiet_job && self.pl.progress.is_empty() => {}
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
            _ => {}
        }
    }

    /// Enter on a plugin: install it, update it, or say why not.
    fn plugin_enter(&mut self) {
        let Some(row) = self.pl.selected_row() else {
            self.notify("No plugins to show.");
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
            if let Some(entry) = row.entry.as_ref() {
                match entry.unpack().and_then(|u| pluginreview::review(u.dir(), Some(&old))) {
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
        let Some(entry) = row.entry.as_ref() else { return };
        let old = update.as_ref().map(|_| self.pl.store.plugin_dir(&row.manifest.id));
        match entry.unpack().and_then(|u| pluginreview::review(u.dir(), old.as_deref())) {
            Ok((m, r)) => {
                self.pl.review = Some(Review {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    update,
                    items: r.items,
                    gone: r.gone,
                    selected: 0,
                    at_start: false,
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

    /// `n` or esc on the review: nothing is installed. An update platoon asked about when it started isn't asked
    /// about again for this version.
    pub(super) fn close_review(&mut self) {
        let r = self.pl.review.take();
        self.mode = self.plugins_base();
        if let Some(r) = r.filter(|r| r.at_start) {
            let (from, to) = r.update.clone().unwrap_or_default();
            match self.pl.store.decline_update(&r.id, &to) {
                Ok(()) => self.notify(format!("{} stays at {from}; platoon won't ask about {to} again. The settings (s, then Plugins) can update it.", r.name)),
                Err(e) => self.notify(format!("{} stays at {from}. ({e})", r.name)),
            }
            self.plugins_changed();
        }
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
        let Some(row) = self.pl.rows().into_iter().find(|r| r.manifest.id == id) else { return };
        let Some(entry) = row.entry.clone() else { return };
        let store = self.pl.store.clone();
        let name = row.manifest.name.clone();
        let updating = row.state == PlugState::Update;
        let dirs = self.ed.dirs.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let r = entry.unpack().and_then(|u| store.install_noting(u.dir(), Origin::Catalog)).map_err(|e| format!("Not installed: {e}")).and_then(|(o, notes)| {
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
        self.pl.quiet_job = false;
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
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ' | 'h' | 'l' | '-' | '+' | '=' | 'r') => {}
            _ => return false,
        }
        true
    }
}
