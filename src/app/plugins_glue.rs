//! The Plugins tab (`4`) and the Settings screen's Plugins tab: the catalog (fetched with git on a thread when the
//! tab is first shown, and again with `f`), the installed plugins, installing and updating on a thread with a line in
//! the footer, removing. The work itself is in `plugins.rs`.

use super::*;
use crate::editor_install::{Dirs, Part, Progress};
use crate::editorplugin;
use crate::plugins::{self, Catalog, Installed, Manifest, Origin, Source, Store};
use std::collections::BTreeMap;
use std::sync::mpsc::TryRecvError;

pub const TOP_PLUGINS: usize = 3;

/// Where a plugin stands, as the Plugins tab shows it.
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

    pub fn installed(&self) -> bool {
        !matches!(self, PlugState::Available | PlugState::Unavailable(_))
    }
}

/// One row of the Plugins tab: a plugin of the catalog, or one installed that the catalog doesn't have.
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
    /// The catalog has been asked for in this run (it is fetched once when the tab is first shown).
    fetch_tried: bool,
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
            job: None,
            job_label: String::new(),
            from_settings: false,
            progress: BTreeMap::new(),
            editor_bytes: None,
            editor_complete: false,
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
            let base = config.parent().unwrap_or(config);
            Self::new(Store::new(base.join("data/plugins")), Source { url: base.join("no-plugin-source").to_string_lossy().into_owned(), rf: "main".into() })
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
}

impl App {
    /// The Plugins tab is showing.
    pub fn plugins_tab_showing(&self) -> bool {
        self.top == TOP_PLUGINS && self.open_project.is_none()
    }

    /// Something of the plugins is running on a thread (the event loop refreshes more often meanwhile).
    pub fn plugins_busy(&self) -> bool {
        self.pl.fetching() || self.pl.working()
    }

    /// Called when the Plugins tab is shown: the catalog is fetched the first time.
    pub(super) fn plugins_shown(&mut self) {
        if !self.pl.fetch_tried {
            self.fetch_catalog();
        }
    }

    /// Fetch the catalog on a thread (`f`, and the first time the tab shows).
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
                    self.pl.catalog = Some(c);
                    self.pl.fetch_error = None;
                    self.pl.fetching = None;
                }
                Ok(Err(e)) => {
                    if self.plugins_tab_showing() {
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
        self.pl.installed = self.pl.store.installed();
        let dirs = self.ed.dirs.clone();
        self.pl.editor_facts(&dirs);
        let n = self.pl.rows().len();
        self.pl.selected = self.pl.selected.min(n.saturating_sub(1));
        self.themes.refresh();
        self.sync_theme();
    }

    fn move_plugin(&mut self, delta: i32) {
        let last = self.pl.rows().len().saturating_sub(1) as i32;
        self.pl.selected = (self.pl.selected as i32 + delta).clamp(0, last) as usize;
    }

    /// A click on a row of the Plugins tab: select it (clicking the selected one again is enter).
    pub(super) fn click_plugin(&mut self, i: usize) {
        if i == self.pl.selected {
            self.plugin_enter();
        } else {
            self.pl.selected = i;
        }
    }

    pub(super) fn plugins_tab_key(&mut self, key: KeyEvent) {
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
        let was = row.installed.map(|p| p.manifest.version).unwrap_or_default();
        self.mode = Mode::Confirm(Confirm::InstallPlugin {
            id: row.manifest.id.clone(),
            name: row.manifest.name.clone(),
            adds: row.manifest.adds_words(),
            update: Some((was, row.manifest.version.clone())),
            download: row.download_bytes,
        });
    }

    /// `d`: take the selected plugin off (on the Plugins tab), or the selected installed one (in the settings).
    pub(super) fn ask_remove_plugin(&mut self, from_settings: bool) {
        let target = if from_settings {
            self.pl.installed.get(self.setting).cloned()
        } else {
            self.pl.selected_row().and_then(|r| r.installed)
        };
        let Some(p) = target else {
            if !from_settings {
                if let Some(row) = self.pl.selected_row() {
                    self.notify(format!("{} isn't installed.", row.manifest.name));
                }
            }
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
        self.mode = Mode::Confirm(Confirm::RemovePlugin { id: p.id().to_string(), name: p.manifest.name.clone(), adds: p.manifest.adds_words(), in_use, editor });
    }

    /// Yes to installing (or updating) a plugin from the catalog: on a thread, with a line in the footer.
    pub(super) fn install_plugin(&mut self, id: &str) {
        self.mode = Mode::List;
        let (Some(c), Some(row)) = (self.pl.catalog.clone(), self.pl.rows().into_iter().find(|r| r.manifest.id == id)) else { return };
        let Some(dir) = row.entry.clone() else { return };
        let store = self.pl.store.clone();
        let origin = Origin::Catalog { source: c.source.clone(), rf: c.rf.clone(), commit: c.commit.clone() };
        let name = row.manifest.name.clone();
        let updating = row.state == PlugState::Update;
        let dirs = self.ed.dirs.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let r = store.install(&dir, origin).map_err(|e| format!("Not installed: {e}")).and_then(|o| {
                let m = &o.plugin().manifest;
                let said = match o {
                    plugins::Outcome::Replaced { ref was, .. } if *was != m.version => format!("{} is updated from {was} to {}.", m.name, m.version),
                    plugins::Outcome::Replaced { .. } => format!("{} {} is installed from the catalog.", m.name, m.version),
                    _ if m.editor.is_some() => format!("{} is installed.", m.name),
                    _ => format!("{} is installed: {} added. Pick one in the settings (s, then Theme).", m.name, m.adds_words()),
                };
                if m.editor.is_none() {
                    return Ok(said);
                }
                // The code editor: then what its table has that the editor folder doesn't.
                let progress = tx.clone();
                let report = move |p| {
                    let _ = progress.send(JobMsg::Progress(p));
                };
                match editorplugin::install_downloads(&store, &dirs, &report) {
                    Ok(_) => Ok(format!("{said} v on a session or a ticket opens it.")),
                    Err(e) => Err(format!("{said} Its downloads didn't finish: {e} Enter on it here tries again.")),
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
        self.mode = if std::mem::take(&mut self.pl.from_settings) { Mode::Settings } else { Mode::List };
        let was_using = self.theme.id.starts_with(&format!("{}{id}/", plugins::THEME_PREFIX));
        if editorplugin::is_editor(&self.pl.store, id) {
            let r = editorplugin::remove(&self.pl.store, &self.ed.dirs, id);
            self.plugins_changed();
            self.setting = self.setting.min(self.pl.installed.len().saturating_sub(1));
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
        let r = self.pl.store.remove(id);
        self.plugins_changed();
        self.setting = self.setting.min(self.pl.installed.len().saturating_sub(1));
        match r {
            Ok(Some(p)) if was_using => self.notify(format!("Removed {}. Its theme was in use, so platoon is back to {}.", p.manifest.name, self.theme.name)),
            Ok(Some(p)) => self.notify(format!("Removed {}.", p.manifest.name)),
            Ok(None) => self.notify("That plugin wasn't installed."),
            Err(e) => self.notify(format!("Not removed: {e}")),
        }
    }

    /// No to removing: back where it was asked.
    pub(super) fn cancel_remove_plugin(&mut self) {
        self.mode = if std::mem::take(&mut self.pl.from_settings) { Mode::Settings } else { Mode::List };
    }

    /// The settings screen's Plugins tab: up and down over the installed plugins, `d` removes one.
    pub(super) fn plugin_settings_key(&mut self, key: KeyEvent) -> bool {
        if self.settings_tab != crate::settings::TAB_PLUGINS {
            return false;
        }
        let last = self.pl.installed.len().saturating_sub(1);
        match key.code {
            KeyCode::Up => self.setting = self.setting.saturating_sub(1),
            KeyCode::Down => self.setting = (self.setting + 1).min(last),
            KeyCode::Home | KeyCode::Char('g') => self.setting = 0,
            KeyCode::End | KeyCode::Char('G') => self.setting = last,
            KeyCode::Char('d') | KeyCode::Delete => self.ask_remove_plugin(true),
            KeyCode::Left | KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ' | 'h' | 'l' | '-' | '+' | '=' | 'r') => {}
            _ => return false,
        }
        true
    }
}
