//! platoon's own settings, in `<config dir>/settings.json` next to `archive.json`: the two automatic
//! clean-ups, the color theme, the four limits of the word search, and everything
//! about the features that use the user's Claude tokens (see `ai.rs`). Whether idle stop counts MCP servers is
//! kept here too (`idle_stop_ignores_mcp`) but has no entry on the screen: it is set by hand in the file. The
//! settings screen edits them through `save`, and `TABS` describes what it shows.
//!
//! Like the archive file, this one must never take platoon down or be lost: a missing or unreadable file
//! just means the defaults, values that make no sense are clamped, writes are atomic, and a file that
//! can't be understood is copied to `.bak` before it is ever replaced. Everything that uses a setting
//! re-reads the file when it changes (`SettingsStore::refresh`), so an edit applies without restarting
//! anything, including the session hosts that outlive platoon.

use crate::agents::{Agent, Catalog};
use crate::ai::{FEATURES, MASTER_LABEL};
use crate::archive::keep_backup;
use crate::theme::{DEFAULT_ID, Theme};
use crate::usertheme::Registry;
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Duration, SystemTime},
};

/// A session nobody is looking at is stopped after this long, at most. A week is far past any sane use.
pub const MAX_IDLE_STOP_MINUTES: u64 = 7 * 24 * 60;
/// Longest auto-archive age. Also how long the archive file remembers restores (see archive.rs).
pub const MAX_AUTO_ARCHIVE_DAYS: u64 = 365;

/// Search: the longest message kept per conversation for searching, in characters (default, then limits).
pub const DEFAULT_PROMPT_CHARS: u64 = 2000;
pub const MIN_PROMPT_CHARS: u64 = 200;
pub const MAX_PROMPT_CHARS: u64 = 20_000;
/// Search: the longest preview of a matching message shown in a result, in characters.
pub const DEFAULT_SNIPPET_CHARS: u64 = 120;
pub const MIN_SNIPPET_CHARS: u64 = 30;
pub const MAX_SNIPPET_CHARS: u64 = 400;
/// Search: how many results the full-text search shows.
pub const DEFAULT_SEARCH_RESULTS: u64 = 50;
pub const MIN_SEARCH_RESULTS: u64 = 5;
pub const MAX_SEARCH_RESULTS: u64 = 500;

/// Token features: how many new messages from you a conversation gathers before its summary is refreshed.
pub const DEFAULT_REFRESH_MESSAGES: u64 = 5;
pub const MIN_REFRESH_MESSAGES: u64 = 1;
pub const MAX_REFRESH_MESSAGES: u64 = 100;
/// Token features: how many conversations one call to Claude covers.
pub const DEFAULT_BATCH: u64 = 30;
pub const MIN_BATCH: u64 = 1;
pub const MAX_BATCH: u64 = 100;
/// Token features: the most tokens (all four numbers Claude reports, added up) platoon may use in a day. There is
/// no "unlimited": a bug or a busy day must not be able to empty the plan, so this has a floor as well.
pub const DEFAULT_DAILY_TOKENS: u64 = 500_000;
pub const MIN_DAILY_TOKENS: u64 = 20_000;
pub const MAX_DAILY_TOKENS: u64 = 100_000_000;
/// Token features: background work pauses while the plan is more than this percent used. 0 means never pause.
pub const DEFAULT_PAUSE_PERCENT: u64 = 80;
pub const MIN_PAUSE_PERCENT: u64 = 10;
pub const MAX_PAUSE_PERCENT: u64 = 100;
/// Token features: what each conversation contributes to a naming call: its last few messages, each cut to this many characters.
pub const DEFAULT_PROMPTS_PER_SESSION: u64 = 8;
pub const MIN_PROMPTS_PER_SESSION: u64 = 1;
pub const MAX_PROMPTS_PER_SESSION: u64 = 30;
pub const DEFAULT_PROMPT_SEND_CHARS: u64 = 300;
pub const MIN_PROMPT_SEND_CHARS: u64 = 50;
pub const MAX_PROMPT_SEND_CHARS: u64 = 2000;
/// Token features: a conversation is refreshed only after its transcript has been quiet this many seconds.
pub const DEFAULT_QUIET_SECONDS: u64 = 60;
pub const MAX_QUIET_SECONDS: u64 = 3600;
/// Project notes: how long a project must be quiet before its notes are brought up to date, in minutes.
pub const DEFAULT_NOTES_QUIET_MINUTES: u64 = 3;
pub const MIN_NOTES_QUIET_MINUTES: u64 = 1;
pub const MAX_NOTES_QUIET_MINUTES: u64 = 60;
/// How much of Claude's last reply in each session the notes updater reads (characters).
pub const DEFAULT_NOTES_REPLY_CHARS: u64 = 1500;
pub const MIN_NOTES_REPLY_CHARS: u64 = 200;
pub const MAX_NOTES_REPLY_CHARS: u64 = 5000;
/// Token features: archived conversations may use at most this percent of what is left of the day's limit.
pub const DEFAULT_ARCHIVED_SHARE: u64 = 50;
pub const MIN_ARCHIVED_SHARE: u64 = 5;
pub const MAX_ARCHIVED_SHARE: u64 = 100;
/// Token features: finding by description sends this many candidates (200 for a wider search) and shows this many results.
pub const DEFAULT_FIND_CANDIDATES: u64 = 50;
pub const MIN_FIND_CANDIDATES: u64 = 10;
pub const MAX_FIND_CANDIDATES: u64 = 300;
pub const DEFAULT_FIND_RESULTS: u64 = 10;
pub const MIN_FIND_RESULTS: u64 = 3;
pub const MAX_FIND_RESULTS: u64 = 20;
pub const DEFAULT_FIND_WIDE: u64 = 200;
pub const MIN_FIND_WIDE: u64 = 50;
pub const MAX_FIND_WIDE: u64 = 1000;
/// For the two automatic clean-ups, 0 means the feature is off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Stop a platoon-hosted session with no viewer, not working and silent for this many minutes.
    pub idle_stop_minutes: u64,
    /// Archive a conversation with no running process and no activity for this many days.
    pub auto_archive_days: u64,
    /// Idle stop doesn't count MCP servers as something running under the session. Off: any process does.
    pub idle_stop_ignores_mcp: bool,
    /// The id of the color theme, exactly as the file says. It may be one this version doesn't know (a newer
    /// version's, or a user theme): that shows as the default theme (the registry finds none) but is kept
    /// on disk until the user picks another.
    pub theme: String,
    /// Full-text search keeps at most this many characters of each message you sent (see `prompts.rs`).
    pub search_prompt_chars: u64,
    /// A result of the full-text search shows at most this many characters of the matching message.
    pub search_snippet_chars: u64,
    /// The full-text search shows at most this many conversations.
    pub search_results: u64,
    /// The full-text search looks in archived conversations too.
    pub search_archived: bool,
    /// The master switch for everything that spends the user's Claude tokens. Off unless the user turned it on.
    pub tokens_enabled: bool,
    /// One switch per token feature, by the feature's id (see `ai::FEATURES`). A feature that is missing is off,
    /// unless its row says it is on as soon as the master switch is (`ai::on_by_default`).
    /// Ids this version doesn't know are kept as they are.
    pub token_features: BTreeMap<String, bool>,
    /// `platoon setup` has already asked about the token features, so it doesn't ask again.
    pub tokens_asked: bool,
    /// New messages from you before a conversation's summary is refreshed.
    pub ai_refresh_messages: u64,
    /// Conversations covered by one call.
    pub ai_batch_size: u64,
    /// The most tokens used in one (local calendar) day.
    pub ai_daily_tokens: u64,
    /// Background token work pauses above this percent of the plan; 0 means never.
    pub ai_pause_percent: u64,
    /// The model the smart features use, exactly as the file says (any model of the catalog's, `agents::Catalog`, or
    /// one this computer no longer has, which is said, never swapped for another). None: the default for this
    /// computer (`Catalog::default_smart`).
    pub ai_model: Option<String>,
    /// The model project notes are written with, the same way; None: Sonnet where Claude Code is installed
    /// (`Catalog::default_notes`).
    pub notes_model: Option<String>,
    /// How many of a conversation's latest messages a naming call carries, and how long each may be (characters).
    pub ai_prompts_per_session: u64,
    pub ai_prompt_chars: u64,
    /// A conversation is refreshed only once its transcript has been quiet this many seconds.
    pub ai_quiet_seconds: u64,
    /// Archived sessions are named slowly in the background (`"ai_archived": "background"` in the file) instead of not at all.
    pub ai_archived: bool,
    /// The most of the day's remaining limit archived conversations may use, in percent.
    pub ai_archived_share: u64,
    /// The editor the notes were last opened with: an editor's id (see `editors.rs`), or `"ask"` while none has
    /// been used yet. platoon remembers the last choice here; an id this computer no longer has is simply not
    /// starred and stays in the file until another editor is used.
    pub notes_editor: String,
    /// The favourite editor of `v` (a ticket's or a session's files): the id of an editor of this computer's
    /// (`fileeditors.rs`), or empty while none has been chosen (`v` opens the first one found). One that is no longer
    /// installed is kept here, and `v` opens the first one found instead (saying so). The `platoon` an older platoon
    /// wrote (its own editor, gone) reads as none chosen.
    pub files_editor: String,
    /// `v` shows the list of editors every time (the favourite starred and picked) instead of opening the favourite.
    pub files_editor_ask: bool,
    /// How long a project must be quiet before platoon brings its notes up to date, in minutes.
    pub notes_quiet_minutes: u64,
    /// How much of Claude's last reply in each session a notes update reads, in characters.
    pub notes_reply_chars: u64,
    /// Finding by description: how many conversations one search sends, how many it shows, how many a wider search sends.
    pub ai_find_candidates: u64,
    pub ai_find_results: u64,
    pub ai_find_wide_candidates: u64,
    /// Once a day at most, ask GitHub whether a newer platoon is out (`versions.rs`): platoon's only network request.
    pub update_check: bool,
    /// The agent the user last started a new session in (`n` asks when both are installed): `claude` or `codex` as the
    /// file says (`Agent::id`), picked in advance the next time. None until the user has chosen once; a word platoon
    /// doesn't know reads as none and stays in the file until they choose.
    pub new_session_agent: Option<String>,
    /// Use Claude Code (Settings, General, Programs). Off: platoon leaves it out as if it weren't installed
    /// (`agents::Providers::switched`). On unless the user switched it off.
    pub use_claude_code: bool,
    /// Use Codex, the same way.
    pub use_codex: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            idle_stop_minutes: 120,
            auto_archive_days: 2,
            idle_stop_ignores_mcp: true,
            theme: DEFAULT_ID.into(),
            search_prompt_chars: DEFAULT_PROMPT_CHARS,
            search_snippet_chars: DEFAULT_SNIPPET_CHARS,
            search_results: DEFAULT_SEARCH_RESULTS,
            search_archived: true,
            tokens_enabled: false,
            token_features: BTreeMap::new(),
            tokens_asked: false,
            ai_refresh_messages: DEFAULT_REFRESH_MESSAGES,
            ai_batch_size: DEFAULT_BATCH,
            ai_daily_tokens: DEFAULT_DAILY_TOKENS,
            ai_pause_percent: DEFAULT_PAUSE_PERCENT,
            ai_model: None,
            notes_model: None,
            ai_prompts_per_session: DEFAULT_PROMPTS_PER_SESSION,
            ai_prompt_chars: DEFAULT_PROMPT_SEND_CHARS,
            ai_quiet_seconds: DEFAULT_QUIET_SECONDS,
            ai_archived: false,
            ai_archived_share: DEFAULT_ARCHIVED_SHARE,
            notes_editor: crate::editors::ASK.into(),
            files_editor: String::new(),
            files_editor_ask: false,
            notes_quiet_minutes: DEFAULT_NOTES_QUIET_MINUTES,
            notes_reply_chars: DEFAULT_NOTES_REPLY_CHARS,
            ai_find_candidates: DEFAULT_FIND_CANDIDATES,
            ai_find_results: DEFAULT_FIND_RESULTS,
            ai_find_wide_candidates: DEFAULT_FIND_WIDE,
            update_check: true,
            new_session_agent: None,
            use_claude_code: true,
            use_codex: true,
        }
    }
}

impl Settings {
    pub fn clamped(self) -> Self {
        Self {
            idle_stop_minutes: self.idle_stop_minutes.min(MAX_IDLE_STOP_MINUTES),
            auto_archive_days: self.auto_archive_days.min(MAX_AUTO_ARCHIVE_DAYS),
            search_prompt_chars: self.search_prompt_chars.clamp(MIN_PROMPT_CHARS, MAX_PROMPT_CHARS),
            search_snippet_chars: self.search_snippet_chars.clamp(MIN_SNIPPET_CHARS, MAX_SNIPPET_CHARS),
            search_results: self.search_results.clamp(MIN_SEARCH_RESULTS, MAX_SEARCH_RESULTS),
            ai_refresh_messages: self.ai_refresh_messages.clamp(MIN_REFRESH_MESSAGES, MAX_REFRESH_MESSAGES),
            ai_batch_size: self.ai_batch_size.clamp(MIN_BATCH, MAX_BATCH),
            ai_daily_tokens: self.ai_daily_tokens.clamp(MIN_DAILY_TOKENS, MAX_DAILY_TOKENS),
            ai_pause_percent: if self.ai_pause_percent == 0 {
                0
            } else {
                self.ai_pause_percent.clamp(MIN_PAUSE_PERCENT, MAX_PAUSE_PERCENT)
            },
            ai_prompts_per_session: self.ai_prompts_per_session.clamp(MIN_PROMPTS_PER_SESSION, MAX_PROMPTS_PER_SESSION),
            ai_prompt_chars: self.ai_prompt_chars.clamp(MIN_PROMPT_SEND_CHARS, MAX_PROMPT_SEND_CHARS),
            ai_quiet_seconds: self.ai_quiet_seconds.min(MAX_QUIET_SECONDS),
            ai_archived_share: self.ai_archived_share.clamp(MIN_ARCHIVED_SHARE, MAX_ARCHIVED_SHARE),
            notes_quiet_minutes: self.notes_quiet_minutes.clamp(MIN_NOTES_QUIET_MINUTES, MAX_NOTES_QUIET_MINUTES),
            notes_reply_chars: self.notes_reply_chars.clamp(MIN_NOTES_REPLY_CHARS, MAX_NOTES_REPLY_CHARS),
            ai_find_candidates: self.ai_find_candidates.clamp(MIN_FIND_CANDIDATES, MAX_FIND_CANDIDATES),
            ai_find_results: self.ai_find_results.clamp(MIN_FIND_RESULTS, MAX_FIND_RESULTS),
            ai_find_wide_candidates: self.ai_find_wide_candidates.clamp(MIN_FIND_WIDE, MAX_FIND_WIDE),
            ..self
        }
    }

    /// The programs the user switched off (Settings, General, Programs), Claude Code first.
    pub fn switched_off(&self) -> Vec<Agent> {
        Agent::ALL.into_iter().filter(|&a| !self.uses(a)).collect()
    }

    /// Is `agent` switched on (whether or not it is installed)?
    pub fn uses(&self, agent: Agent) -> bool {
        match agent {
            Agent::Claude => self.use_claude_code,
            Agent::Codex => self.use_codex,
        }
    }

    pub fn set_uses(&mut self, agent: Agent, on: bool) {
        match agent {
            Agent::Claude => self.use_claude_code = on,
            Agent::Codex => self.use_codex = on,
        }
    }

    /// The model the smart features use on a computer with these models: the one picked, else the default.
    pub fn smart_model_in(&self, catalog: &Catalog) -> Option<String> {
        self.ai_model.clone().filter(|m| !m.is_empty()).or_else(|| catalog.default_smart())
    }

    /// The model project notes are written with on a computer with these models: the one picked, else the default.
    pub fn notes_model_in(&self, catalog: &Catalog) -> Option<String> {
        self.notes_model.clone().filter(|m| !m.is_empty()).or_else(|| catalog.default_notes())
    }

    /// The model a call for `feature` uses when it names none of its own, and the setting that picks it: project notes
    /// have one of their own; everything else uses the smart features' one.
    pub fn model_for(&self, feature: &str, catalog: &Catalog) -> (Option<String>, ModelFor) {
        if feature == crate::notesrun::FEATURE {
            (self.notes_model_in(catalog), ModelFor::Notes)
        } else {
            (self.smart_model_in(catalog), ModelFor::Smart)
        }
    }

    /// The feature's own switch, whatever the master says.
    pub fn feature_toggle(&self, id: &str) -> bool {
        self.token_features.get(id).copied().unwrap_or_else(|| crate::ai::on_by_default(id))
    }

    pub fn set_feature(&mut self, id: &str, on: bool) {
        self.token_features.insert(id.to_string(), on);
    }

    /// A feature may spend tokens only when the master switch AND its own switch are on.
    pub fn feature_on(&self, id: &str) -> bool {
        self.tokens_enabled && self.feature_toggle(id)
    }

    /// The idle-stop threshold, or None when it is off. `secs_per_minute` is 60 outside of tests.
    pub fn idle_stop_after(&self, secs_per_minute: u64) -> Option<Duration> {
        (self.idle_stop_minutes > 0).then(|| Duration::from_secs(self.idle_stop_minutes * secs_per_minute))
    }

    /// The theme to draw with: the chosen one (a built-in or one of the user's), or the default when the id isn't
    /// known, for instance a user theme whose file has since been deleted.
    pub fn theme(&self, themes: &Registry) -> Theme {
        themes.by_id(&self.theme).unwrap_or_default()
    }

    /// The auto-archive age in seconds, or None when it is off.
    pub fn auto_archive_secs(&self) -> Option<u64> {
        (self.auto_archive_days > 0).then(|| self.auto_archive_days * 86_400)
    }
}

/// None if the text isn't a JSON object. A key that is missing or isn't a non-negative number (or, for the
/// yes/no one, a boolean) keeps its default (a negative number is nonsense, not "off"), and numbers past the limits are clamped.
fn parse(text: &str) -> Option<Settings> {
    let v: Value = serde_json::from_str(text).ok()?;
    let o = v.as_object()?;
    let d = Settings::default();
    let number = |key: &str, default: u64| match o.get(key) {
        Some(v) => v.as_u64().or_else(|| v.as_f64().filter(|f| f.is_finite() && *f >= 0.0).map(|f| f as u64)).unwrap_or(default),
        None => default,
    };
    // Only a real true/false counts: anything else (a number, text) is not a choice, so the default stays.
    let ignores_mcp = o.get("idle_stop_ignores_mcp").and_then(Value::as_bool).unwrap_or(d.idle_stop_ignores_mcp);
    let theme = o.get("theme").and_then(Value::as_str).map_or(d.theme.clone(), |id| built_in_pack_id(id).to_string());
    Some(
        Settings {
            idle_stop_minutes: number("idle_stop_minutes", d.idle_stop_minutes),
            auto_archive_days: number("auto_archive_days", d.auto_archive_days),
            idle_stop_ignores_mcp: ignores_mcp,
            theme,
            search_prompt_chars: number("search_prompt_chars", d.search_prompt_chars),
            search_snippet_chars: number("search_snippet_chars", d.search_snippet_chars),
            search_results: number("search_results", d.search_results),
            search_archived: o.get("search_archived").and_then(Value::as_bool).unwrap_or(d.search_archived),
            tokens_enabled: o.get("tokens_enabled").and_then(Value::as_bool).unwrap_or(d.tokens_enabled),
            token_features: o
                .get("token_features")
                .and_then(Value::as_object)
                .map(|f| f.iter().filter_map(|(k, v)| Some((k.clone(), v.as_bool()?))).collect())
                .unwrap_or_default(),
            tokens_asked: o.get("tokens_asked").and_then(Value::as_bool).unwrap_or(d.tokens_asked),
            ai_refresh_messages: number("ai_refresh_messages", d.ai_refresh_messages),
            ai_batch_size: number("ai_batch_size", d.ai_batch_size),
            ai_daily_tokens: number("ai_daily_tokens", d.ai_daily_tokens),
            ai_pause_percent: number("ai_pause_percent", d.ai_pause_percent),
            ai_model: o.get("ai_model").and_then(Value::as_str).filter(|m| !m.trim().is_empty()).map(str::to_string),
            notes_model: o.get("notes_model").and_then(Value::as_str).filter(|m| !m.trim().is_empty()).map(str::to_string),
            ai_prompts_per_session: number("ai_prompts_per_session", d.ai_prompts_per_session),
            ai_prompt_chars: number("ai_prompt_chars", d.ai_prompt_chars),
            ai_quiet_seconds: number("ai_quiet_seconds", d.ai_quiet_seconds),
            // "background" (or a hand-written true) turns it on; "off", anything else, and a missing key mean off.
            ai_archived: match o.get("ai_archived") {
                Some(Value::String(s)) => s == "background",
                Some(Value::Bool(b)) => *b,
                _ => d.ai_archived,
            },
            ai_archived_share: number("ai_archived_share", d.ai_archived_share),
            notes_editor: o.get("notes_editor").and_then(Value::as_str).map_or(d.notes_editor.clone(), str::to_string),
            files_editor: o
                .get("files_editor")
                .and_then(Value::as_str)
                .filter(|v| !v.trim().is_empty() && *v != OLD_PLATOON_EDITOR)
                .map_or(d.files_editor.clone(), str::to_string),
            files_editor_ask: o.get("files_editor_ask").and_then(Value::as_bool).unwrap_or(d.files_editor_ask),
            notes_quiet_minutes: number("notes_quiet_minutes", d.notes_quiet_minutes),
            notes_reply_chars: number("notes_reply_chars", d.notes_reply_chars),
            ai_find_candidates: number("ai_find_candidates", d.ai_find_candidates),
            ai_find_results: number("ai_find_results", d.ai_find_results),
            ai_find_wide_candidates: number("ai_find_wide_candidates", d.ai_find_wide_candidates),
            update_check: o.get("update_check").and_then(Value::as_bool).unwrap_or(d.update_check),
            new_session_agent: o.get("new_session_agent").and_then(Value::as_str).map(str::to_string),
            use_claude_code: o.get("use_claude_code").and_then(Value::as_bool).unwrap_or(d.use_claude_code),
            use_codex: o.get("use_codex").and_then(Value::as_bool).unwrap_or(d.use_codex),
        }
        .clamped(),
    )
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

/// The id an older platoon gave its own code editor in `files_editor`.
const OLD_PLATOON_EDITOR: &str = "platoon";

/// A theme of the Theme pack plugin platoon once had (`plugin:theme-pack/<slug>`) is now the built-in `<slug>`, so
/// anyone who picked one keeps it. Any other id as it is.
fn built_in_pack_id(id: &str) -> &str {
    id.strip_prefix("plugin:theme-pack/").unwrap_or(id)
}

/// The settings on disk. Never fails: whatever can't be read is the defaults. The second value says the
/// file exists but couldn't be understood.
pub fn load(dir: &Path) -> (Settings, bool) {
    match fs::read_to_string(path_in(dir)) {
        Ok(text) => match parse(&text) {
            Some(s) => (s, false),
            None => (Settings::default(), true),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => (Settings::default(), false),
        Err(_) => (Settings::default(), path_in(dir).exists()),
    }
}

/// The top-level object of the file as it is now, if it holds one. `save` starts from it so that keys
/// platoon doesn't know about (a newer version's, or the user's own notes) survive an edit.
fn existing_object(path: &Path) -> Option<Map<String, Value>> {
    match serde_json::from_str(&fs::read_to_string(path).ok()?).ok()? {
        Value::Object(o) => Some(o),
        _ => None,
    }
}

/// Write the settings (clamped) atomically: a temp file, then a rename, so a crash never leaves half a
/// file. Keys other than the settings themselves are kept as they were. A file that was there but couldn't
/// be understood is kept as `settings.json.bak` first, and replaced by a fresh one.
pub fn save(dir: &Path, settings: &Settings) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = path_in(dir);
    let corrupt = load(dir).1;
    if corrupt {
        keep_backup(&path)?;
    }
    let mut object = if corrupt { Map::new() } else { existing_object(&path).unwrap_or_default() };
    let s = settings.clone().clamped();
    object.entry("version").or_insert(json!(1));
    object.insert("idle_stop_minutes".into(), json!(s.idle_stop_minutes));
    object.insert("auto_archive_days".into(), json!(s.auto_archive_days));
    object.insert("idle_stop_ignores_mcp".into(), json!(s.idle_stop_ignores_mcp));
    object.insert("search_prompt_chars".into(), json!(s.search_prompt_chars));
    object.insert("search_snippet_chars".into(), json!(s.search_snippet_chars));
    object.insert("search_results".into(), json!(s.search_results));
    object.insert("search_archived".into(), json!(s.search_archived));
    object.insert("tokens_enabled".into(), json!(s.tokens_enabled));
    object.insert("tokens_asked".into(), json!(s.tokens_asked));
    object.insert("ai_refresh_messages".into(), json!(s.ai_refresh_messages));
    object.insert("ai_batch_size".into(), json!(s.ai_batch_size));
    object.insert("ai_daily_tokens".into(), json!(s.ai_daily_tokens));
    object.insert("ai_pause_percent".into(), json!(s.ai_pause_percent));
    // A model is written only once one has been picked: until then the default follows what this computer has.
    if let Some(m) = &s.ai_model {
        object.insert("ai_model".into(), json!(m));
    }
    if let Some(m) = &s.notes_model {
        object.insert("notes_model".into(), json!(m));
    }
    object.insert("ai_prompts_per_session".into(), json!(s.ai_prompts_per_session));
    object.insert("ai_prompt_chars".into(), json!(s.ai_prompt_chars));
    object.insert("ai_quiet_seconds".into(), json!(s.ai_quiet_seconds));
    object.insert("ai_archived".into(), json!(if s.ai_archived { "background" } else { "off" }));
    object.insert("ai_archived_share".into(), json!(s.ai_archived_share));
    object.insert("notes_editor".into(), json!(s.notes_editor));
    object.insert("files_editor".into(), json!(s.files_editor));
    object.insert("files_editor_ask".into(), json!(s.files_editor_ask));
    object.insert("notes_quiet_minutes".into(), json!(s.notes_quiet_minutes));
    object.insert("notes_reply_chars".into(), json!(s.notes_reply_chars));
    object.insert("ai_find_candidates".into(), json!(s.ai_find_candidates));
    object.insert("ai_find_results".into(), json!(s.ai_find_results));
    object.insert("ai_find_wide_candidates".into(), json!(s.ai_find_wide_candidates));
    object.insert("update_check".into(), json!(s.update_check));
    object.insert("use_claude_code".into(), json!(s.use_claude_code));
    object.insert("use_codex".into(), json!(s.use_codex));
    // Written only once the user has chosen (`n`'s question): until then nothing is remembered.
    if let Some(a) = &s.new_session_agent {
        object.insert("new_session_agent".into(), json!(a));
    }
    // The per-feature switches: the ones we hold are written, and entries of the file's own that we don't hold
    // (a feature of a newer version) stay.
    let mut features = match object.get("token_features") {
        Some(Value::Object(f)) => f.clone(),
        _ => Map::new(),
    };
    for (id, on) in &s.token_features {
        features.insert(id.clone(), json!(on));
    }
    object.insert("token_features".into(), Value::Object(features));
    // The theme is only written when it is a different choice from what the file already says: a value platoon
    // can't read (an id from another version, or something that isn't text) is the user's until they pick.
    let same = object.get("theme").is_some_and(|v| v.as_str().map_or(s.theme == DEFAULT_ID, |on_disk| on_disk == s.theme));
    if !same {
        object.insert("theme".into(), json!(s.theme));
    }
    let text = serde_json::to_string_pretty(&Value::Object(object)).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    let written = fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// How an entry is changed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// One of a few steps: a number of minutes or days, or a choice between two things.
    Steps,
    /// The color theme. Its choices are the built-in and user themes, which are only known at run time, so
    /// `choices`, `get`, `set` and `label` are unused and the app steps through its theme list instead;
    /// `enter` opens the theme picker.
    Theme,
    /// The system prompt of one AI action (`instructions::ACTIONS`), by id. It has no steps: enter (or a click on
    /// the value) opens the user's own text in an editor, and the value says "Enter to change" or "User Modified".
    Instructions(&'static str),
    /// Which editor opens something, chosen among the editors this computer has (known only at run time, like the
    /// themes): the app steps through them, and `choices`, `get`, `set` and `label` are unused.
    Editor(EditorFor),
    /// The on/off switch of one token feature (`ai::FEATURES`), by id. It reads and writes `Settings::token_features`
    /// instead of a field, so a new feature is a new row in that table and nothing else here.
    Feature(&'static str),
    /// A model, chosen among the models of the AI programs this computer has (`agents::Catalog`, known only at run
    /// time, like the themes): the app steps through them, and `choices`, `get`, `set` and `label` are unused.
    Model(ModelFor),
    /// Whether platoon uses one program (Claude Code or Codex): On or Off, or "Not installed" (which can't be changed).
    /// What it can be depends on what this computer has, so the app reads and changes it (`App::program_setting`), and
    /// `choices`, `get`, `set` and `label` are unused.
    Program(Agent),
}

/// What a model setting picks the model of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModelFor {
    /// The smart features, the hand-off and tasks drafted from a conversation: `Settings::ai_model`.
    Smart,
    /// Project notes: `Settings::notes_model`.
    Notes,
}

impl ModelFor {
    /// The setting's name, as the settings screen shows it.
    pub fn setting_name(self) -> &'static str {
        match self {
            ModelFor::Smart => SMART_MODEL_NAME,
            ModelFor::Notes => NOTES_MODEL_NAME,
        }
    }

    pub fn set(self, s: &mut Settings, model: String) {
        match self {
            ModelFor::Smart => s.ai_model = Some(model),
            ModelFor::Notes => s.notes_model = Some(model),
        }
    }

    /// The model in effect on a computer with these models: the one picked, else the default.
    pub fn in_effect(self, s: &Settings, catalog: &Catalog) -> Option<String> {
        match self {
            ModelFor::Smart => s.smart_model_in(catalog),
            ModelFor::Notes => s.notes_model_in(catalog),
        }
    }
}

/// Said under the model settings while the model in effect is one of Codex's: whatever the call asks, Codex sends its own
/// instructions with it (measured on Codex 0.163: a one-word answer cost 13,287 tokens). The guide says the same
/// (`guide/tokens.md`, "Calls on Codex").
pub const CODEX_COST: &str = "A call on Codex costs about 13,000 to 20,000 tokens of Codex's own instructions, however small the question.";

/// The two program switches' names on the settings screen.
pub const USE_CLAUDE_CODE: &str = "Use Claude Code";
pub const USE_CODEX: &str = "Use Codex";

/// The two model settings' names on the settings screen.
pub const SMART_MODEL_NAME: &str = "Model for smart features";
pub const NOTES_MODEL_NAME: &str = "Model for project notes";

/// What an editor setting chooses the editor for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditorFor {
    /// platoon's own text: a project's notes, a ticket's body, a system prompt (`e`); `Settings::notes_editor`.
    Text,
    /// A ticket's or a session's files (`v`); `Settings::files_editor`.
    Files,
}

/// One line of the settings screen. The screen is built from `TABS` alone, so a new setting is a new entry
/// on one of its tabs (plus its field in `Settings`, `parse` and `save`).
pub struct Item {
    pub name: &'static str,
    /// One plain-language sentence or two: what it does and why nothing is lost.
    pub explain: &'static str,
    pub kind: Kind,
    /// The steps offered, ascending, in the setting's own unit. For a number 0 is "Off"; a choice between
    /// two things is 0 and 1.
    pub choices: &'static [u64],
    pub get: fn(&Settings) -> u64,
    pub set: fn(&mut Settings, u64),
    /// How a value reads: "Off", "2 hours". Also used for a value that is not one of the steps.
    pub label: fn(u64) -> String,
    /// The value that means "switched off" and is drawn muted, if there is one.
    pub off: Option<u64>,
}

fn plural(n: u64, unit: &str) -> String {
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

pub fn minutes_label(m: u64) -> String {
    match m {
        0 => "Off".into(),
        m if m % 1440 == 0 => plural(m / 1440, "day"),
        m if m % 60 == 0 => plural(m / 60, "hour"),
        m => plural(m, "minute"),
    }
}

pub fn days_label(d: u64) -> String {
    if d == 0 { "Off".into() } else { plural(d, "day") }
}

pub fn chars_label(n: u64) -> String {
    plural(n, "character")
}

pub fn results_label(n: u64) -> String {
    plural(n, "result")
}

pub fn on_off(v: u64) -> String {
    if v == 0 { "Off".into() } else { "On".into() }
}

/// A count of tokens as a person says it: 250K, 1M, and the exact number when it is neither.
pub fn tokens_label(n: u64) -> String {
    match n {
        n if n >= 1_000_000 && n % 1_000_000 == 0 => format!("{}M tokens", n / 1_000_000),
        n if n >= 1000 && n % 1000 == 0 => format!("{}K tokens", n / 1000),
        n => plural(n, "token"),
    }
}

pub fn seconds_label(n: u64) -> String {
    match n {
        0 => "No wait".into(),
        n if n % 60 == 0 => plural(n / 60, "minute"),
        n => plural(n, "second"),
    }
}

pub fn percent_label(n: u64) -> String {
    if n == 0 { "Never".into() } else { format!("{n}%") }
}

pub fn messages_label(n: u64) -> String {
    plural(n, "message")
}

pub fn conversations_label(n: u64) -> String {
    plural(n, "session")
}

/// 1234567 -> "1,234,567".
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The tabs of the settings screen, in order, each with its entries in order. The screen is built from this alone.
pub static TABS: LazyLock<Vec<Tab>> = LazyLock::new(tabs);

/// One tab of the settings screen: its title and its entries.
pub struct Tab {
    pub title: &'static str,
    pub items: Vec<Item>,
    /// A heading drawn above the entry at that position, grouping it with the ones after it.
    pub headings: &'static [(usize, &'static str)],
}

impl Tab {
    /// The heading above entry `i`, if one starts a group there.
    pub fn heading(&self, i: usize) -> Option<&'static str> {
        self.headings.iter().find(|(at, _)| *at == i).map(|(_, h)| *h)
    }
}

/// The tabs by position, for the code that treats one of them specially (see `tabs`).
pub const TAB_SMART: usize = 2;
pub const TAB_SESSIONS: usize = 3;
pub const TAB_PROJECTS: usize = 4;
#[cfg(test)]
pub const TAB_EDITORS: usize = 5;

/// Every entry on every tab, in screen order.
#[cfg(test)]
pub fn all_items() -> impl Iterator<Item = &'static Item> {
    TABS.iter().flat_map(|t| t.items.iter())
}

/// The tab and the position on it of the entry called `name`.
#[cfg(test)]
pub fn find_item(name: &str) -> Option<(usize, usize)> {
    TABS.iter().enumerate().find_map(|(t, tab)| tab.items.iter().position(|i| i.name == name).map(|i| (t, i)))
}

fn tabs() -> Vec<Tab> {
    vec![
        Tab { title: "General", items: general_items(), headings: &[(0, "Programs"), (2, "Everything else")] },
        Tab { title: "Search", items: search_items(), headings: &[] },
        Tab { title: "Smart features", items: smart_items(), headings: &[] },
        Tab { title: "Sessions", items: sessions_items(), headings: &[] },
        Tab { title: "Projects", items: projects_items(), headings: &[] },
        Tab { title: "Editors", items: editors_items(), headings: &[(0, "Editing platoon's text"), (1, "Editing files")] },
    ]
}

/// Which editors open what: platoon's own text (notes, ticket bodies, system prompts), and a ticket's or a session's
/// files (`v`).
fn editors_items() -> Vec<Item> {
    let editor = |name, explain, which| Item {
        name,
        explain,
        kind: Kind::Editor(which),
        choices: &[],
        get: |_| 0,
        set: |_, _| {},
        label: |_| String::new(),
        off: None,
    };
    vec![
        editor(
            "Editor for notes and tasks (e)",
            "Opens a project's notes, a task's text and the system prompts (e). It is the one starred in the list of \
             editors e shows; choosing another there changes it too.",
            EditorFor::Text,
        ),
        editor(
            "Editor for folders (v)",
            "Opens a task's or a session's folders (v): one of the editors this computer has that show a file tree. \
             Choosing another from v's list changes it too.",
            EditorFor::Files,
        ),
        Item {
            name: "Always ask which editor",
            explain: "Off: v opens the editor above at once. On: v shows the list every time, with that one starred and picked.",
            kind: Kind::Steps,
            choices: &[0, 1],
            get: |s| u64::from(s.files_editor_ask),
            set: |s, v| s.files_editor_ask = v != 0,
            label: on_off,
            off: Some(0),
        },
    ]
}

/// The entry that picks a model.
fn model_item(which: ModelFor, explain: &'static str) -> Item {
    Item {
        name: which.setting_name(),
        explain,
        kind: Kind::Model(which),
        choices: &[],
        get: |_| 0,
        set: |_, _| {},
        label: |_| String::new(),
        off: None,
    }
}

/// The entry that edits the instructions of one AI action.
fn instructions_item(id: &'static str) -> Item {
    let a = crate::instructions::action(id);
    Item {
        name: a.entry,
        explain: a.explain,
        kind: Kind::Instructions(id),
        choices: &[],
        get: |_| 0,
        set: |_, _| {},
        label: |_| String::new(),
        off: None,
    }
}

/// The two programs platoon can use, then everyday behaviour: the colors, the two automatic clean-ups and the check for
/// a newer platoon.
fn general_items() -> Vec<Item> {
    let program = |name, explain, agent| Item {
        name,
        explain,
        kind: Kind::Program(agent),
        choices: &[],
        get: |_| 0,
        set: |_, _| {},
        label: |_| String::new(),
        off: None,
    };
    vec![
        program(
            USE_CLAUDE_CODE,
            "Off: platoon leaves Claude Code out, as if it weren't installed. Its sessions leave the lists and its models \
             every model choice, and its sessions get no project notes. Sessions and tasks already running keep running.",
            Agent::Claude,
        ),
        program(
            USE_CODEX,
            "Off: platoon leaves Codex out, as if it weren't installed. Its sessions leave the lists and its models every \
             model choice, and its sessions get no project notes. Sessions and tasks already running keep running.",
            Agent::Codex,
        ),
        Item {
            name: "Theme",
            explain: "The colors platoon is drawn in. Press enter to try them on your real screen: up and down preview one, \
                      enter keeps it, esc undoes it.",
            kind: Kind::Theme,
            choices: &[],
            get: |_| 0,
            set: |_, _| {},
            label: |_| String::new(),
            off: None,
        },
        Item {
            name: "Stop idle sessions after",
            explain: "A session platoon is running that nobody has looked at, and that isn't working, is stopped to free memory. \
                      Nothing is lost: opening it again resumes it from its saved history.",
            kind: Kind::Steps,
            choices: &[0, 30, 60, 120, 240, 480, 1440],
            get: |s| s.idle_stop_minutes,
            set: |s, v| s.idle_stop_minutes = v,
            label: minutes_label,
            off: Some(0),
        },
        Item {
            name: "Archive sessions after",
            explain: "A session nobody has used for this long, with nothing running, moves to the Archived tab. \
                      You can restore it from there.",
            kind: Kind::Steps,
            choices: &[0, 1, 2, 7, 14, 30],
            get: |s| s.auto_archive_days,
            set: |s, v| s.auto_archive_days = v,
            label: days_label,
            off: Some(0),
        },
        Item {
            name: "Check for a newer platoon",
            explain: "Once a day at most, platoon asks GitHub for the number of its latest version (it sends only its own version \
                      number) and says once when a newer one is out. Off: platoon makes no network request at all. Applies from the \
                      next start.",
            kind: Kind::Steps,
            choices: &[0, 1],
            get: |s| u64::from(s.update_check),
            set: |s, v| s.update_check = v != 0,
            label: |v| if v == 0 { "Off".into() } else { "Once a day".into() },
            off: Some(0),
        },
    ]
}

/// The word search and the search by description.
fn search_items() -> Vec<Item> {
    vec![
        Item {
            name: "Search archived",
            explain: "Whether searching by the words you typed (press / then tab) also looks in sessions on the Archived tab. \
                      Choosing one from the results opens the Archived tab for you.",
            kind: Kind::Steps,
            choices: &[0, 1],
            get: |s| u64::from(s.search_archived),
            set: |s, v| s.search_archived = v != 0,
            label: |v| if v == 0 { "Skip archived".into() } else { "Include archived".into() },
            off: None,
        },
        Item {
            name: "Search results shown",
            explain: "The most sessions a word search lists. A narrower search finds what you want among fewer.",
            kind: Kind::Steps,
            choices: &[10, 20, 50, 100, 200],
            get: |s| s.search_results,
            set: |s, v| s.search_results = v,
            label: results_label,
            off: None,
        },
        Item {
            name: "Search preview length",
            explain: "The most of a matching message a search result shows around the words you typed. \
                      It is also cut to fit the window.",
            kind: Kind::Steps,
            choices: &[40, 80, 120, 200, 400],
            get: |s| s.search_snippet_chars,
            set: |s, v| s.search_snippet_chars = v,
            label: chars_label,
            off: None,
        },
        Item {
            name: "Longest message kept",
            explain: "How much of each message you sent is remembered for searching, so a huge paste doesn't fill the memory. \
                      Changing it makes platoon read your sessions again in the background.",
            kind: Kind::Steps,
            choices: &[300, 1000, 2000, 5000, 20_000],
            get: |s| s.search_prompt_chars,
            set: |s, v| s.search_prompt_chars = v,
            label: chars_label,
            off: None,
        },
        Item {
            name: "Description search: sessions sent",
            explain: "How many sessions a search by description sends to Claude to pick from. More finds the odd one but uses more of your plan.",
            kind: Kind::Steps,
            choices: &[20, 30, 50, 100, 150],
            get: |s| s.ai_find_candidates,
            set: |s, v| s.ai_find_candidates = v,
            label: conversations_label,
            off: None,
        },
        Item {
            name: "Description search: results",
            explain: "The most sessions a search by description lists.",
            kind: Kind::Steps,
            choices: &[5, 10, 15, 20],
            get: |s| s.ai_find_results,
            set: |s, v| s.ai_find_results = v,
            label: results_label,
            off: None,
        },
        Item {
            name: "Description search: wider search",
            explain: "How many sessions the wider search sends. It asks you first and says roughly how many tokens that is.",
            kind: Kind::Steps,
            choices: &[100, 200, 300, 500],
            get: |s| s.ai_find_wide_candidates,
            set: |s, v| s.ai_find_wide_candidates = v,
            label: conversations_label,
            off: None,
        },
        instructions_item(crate::instructions::FIND),
    ]
}

/// Everything that decides whether the user's Claude plan is used at all: the master switch, one switch per feature
/// (from `ai::FEATURES`), and the limits on the day's use.
fn smart_items() -> Vec<Item> {
    let mut v = Vec::new();
    v.push(Item {
        name: MASTER_LABEL,
        explain: "Lets platoon use your own plan (Claude's, or Codex's for an OpenAI model) for the smart features below. It sends \
                  the messages you typed plus part of the last reply, in small batches. While this is Off nothing is sent, and turning \
                  it Off stops any call in progress.",
        kind: Kind::Steps,
        choices: &[0, 1],
        get: |s| u64::from(s.tokens_enabled),
        set: |s, v| s.tokens_enabled = v != 0,
        label: on_off,
        off: Some(0),
    });
    for f in FEATURES {
        v.push(Item {
            name: f.label,
            explain: f.description,
            kind: Kind::Feature(f.id),
            choices: &[0, 1],
            get: |_| 0,
            set: |_, _| {},
            label: on_off,
            off: Some(0),
        });
    }
    v.extend([
        Item {
            name: "Daily token limit",
            explain: "The most tokens the smart features may use in one day, counting everything reported, on Claude Code and \
                      on Codex each. When it is reached they wait until tomorrow. Saved summaries stay.",
            kind: Kind::Steps,
            choices: &[100_000, 250_000, 500_000, 1_000_000, 2_000_000, 5_000_000],
            get: |s| s.ai_daily_tokens,
            set: |s, v| s.ai_daily_tokens = v,
            label: tokens_label,
            off: None,
        },
        Item {
            name: "Pause above plan usage",
            explain: "Smart features stop while the plan they would use is used more than this much, so your own work isn't held \
                      up: Claude's as its sessions report it, Codex's as its sessions saved it.",
            kind: Kind::Steps,
            choices: &[0, 50, 60, 70, 80, 90],
            get: |s| s.ai_pause_percent,
            set: |s, v| s.ai_pause_percent = v,
            label: percent_label,
            off: Some(0),
        },
        model_item(
            ModelFor::Smart,
            "Names and summaries, the description search, project descriptions, the hand-off and tasks from a conversation. \
             Claude's models run on Claude Code, OpenAI's on Codex. Haiku is Claude's smallest and uses the least of your plan.",
        ),
    ]);
    v
}

/// What naming and summarizing the conversations reads, and how often.
fn sessions_items() -> Vec<Item> {
    vec![
        Item {
            name: "Sessions per call",
            explain: "Names and summaries are made for several sessions at once, in one request to Claude. This is how many go in each request. More per request uses less of your plan in total, because the instructions are sent once for all of them.",
            kind: Kind::Steps,
            choices: &[5, 10, 20, 30, 50, 100],
            get: |s| s.ai_batch_size,
            set: |s, v| s.ai_batch_size = v,
            label: conversations_label,
            off: None,
        },
        Item {
            name: "Refresh a summary after",
            explain: "How many new messages from you a session gets before its name and summary are made again.",
            kind: Kind::Steps,
            choices: &[1, 3, 5, 10, 20, 50],
            get: |s| s.ai_refresh_messages,
            set: |s, v| s.ai_refresh_messages = v,
            label: messages_label,
            off: None,
        },
        Item {
            name: "Wait for quiet before refreshing",
            explain: "Before making a session's name and summary again, platoon waits until nothing new has been added to it for this long. That way a conversation you're still in isn't summarized again after every reply.",
            kind: Kind::Steps,
            choices: &[15, 30, 60, 120, 300, 600],
            get: |s| s.ai_quiet_seconds,
            set: |s, v| s.ai_quiet_seconds = v,
            label: seconds_label,
            off: None,
        },
        Item {
            name: "Recent messages sent",
            explain: "How many of a session's latest messages from you go into its summary, besides the first one. More of them names it better and uses more of your plan.",
            kind: Kind::Steps,
            choices: &[3, 5, 8, 12, 20],
            get: |s| s.ai_prompts_per_session,
            set: |s, v| s.ai_prompts_per_session = v,
            label: messages_label,
            off: None,
        },
        Item {
            name: "Longest message sent",
            explain: "Each message is cut to this many characters before it is sent, so a huge paste doesn't use up your plan.",
            kind: Kind::Steps,
            choices: &[100, 200, 300, 500, 1000],
            get: |s| s.ai_prompt_chars,
            set: |s, v| s.ai_prompt_chars = v,
            label: chars_label,
            off: None,
        },
        Item {
            name: "Archived sessions",
            explain: "Whether sessions on the Archived tab get names and summaries too, slowly in the background. One that you restore or open is done at once either way.",
            kind: Kind::Steps,
            choices: &[0, 1],
            get: |s| u64::from(s.ai_archived),
            set: |s, v| s.ai_archived = v != 0,
            label: |v| if v == 0 { "Skip".into() } else { "In the background".into() },
            off: Some(0),
        },
        Item {
            name: "Share for archived",
            explain: "Naming archived sessions may use at most this much of what is left of the day's limit, so the sessions you are using always have room.",
            kind: Kind::Steps,
            choices: &[10, 25, 50, 75, 100],
            get: |s| s.ai_archived_share,
            set: |s, v| s.ai_archived_share = v,
            label: |v| format!("{v}%"),
            off: None,
        },
        instructions_item(crate::instructions::NAMES),
    ]
}

/// Keeping a project's notes up to date.
fn projects_items() -> Vec<Item> {
    vec![
        Item {
            name: "Update notes after",
            explain: "How long a project has to be quiet before platoon brings its notes up to date. A new answer in any of its \
                      sessions starts the wait again, so it only writes once the work has stopped.",
            kind: Kind::Steps,
            choices: &[1, 3, 5, 15, 30],
            get: |s| s.notes_quiet_minutes,
            set: |s, v| s.notes_quiet_minutes = v,
            label: minutes_label,
            off: None,
        },
        Item {
            name: "Read Claude's last reply up to",
            explain: "How much of Claude's last answer in each session platoon reads when it brings a project's notes up to date. \
                      What is still open is usually in Claude's answers; more of them uses a little more of your plan.",
            kind: Kind::Steps,
            choices: &[500, 1500, 3000],
            get: |s| s.notes_reply_chars,
            set: |s, v| s.notes_reply_chars = v,
            label: chars_label,
            off: None,
        },
        model_item(
            ModelFor::Notes,
            "The model that keeps project notes up to date: unless you pick another, Sonnet where Claude Code is installed, \
             else Codex's first. Claude's models run on Claude Code, OpenAI's on Codex.",
        ),
        instructions_item(crate::instructions::NOTES),
        instructions_item(crate::instructions::DESCRIPTION),
        instructions_item(crate::instructions::HANDOFF),
        instructions_item(crate::instructions::TASK_DRAFT),
    ]
}

/// Which way to move through an item's choices.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Prev,
    Next,
    /// Next, and back to the first after the last.
    Cycle,
}

impl Item {
    /// The entry's current value in the numbers `choices` are written in (a switch is 0 or 1).
    pub fn value(&self, settings: &Settings) -> u64 {
        match self.kind {
            Kind::Feature(id) => u64::from(settings.feature_toggle(id)),
            _ => (self.get)(settings),
        }
    }

    pub fn assign(&self, settings: &mut Settings, value: u64) {
        match self.kind {
            Kind::Feature(id) => settings.set_feature(id, value != 0),
            _ => (self.set)(settings, value),
        }
    }

    /// The choice `step` leads to from `value`, which needn't be one of the choices (a hand-edited file):
    /// it goes to the nearest step in that direction. Prev and Next stop at the ends; Cycle wraps.
    pub fn step(&self, value: u64, step: Step) -> u64 {
        let next = self.choices.iter().copied().find(|&c| c > value);
        let prev = self.choices.iter().rev().copied().find(|&c| c < value);
        match step {
            Step::Prev => prev.unwrap_or(value),
            Step::Next => next.unwrap_or(value),
            Step::Cycle => next.or(self.choices.first().copied()).unwrap_or(value),
        }
    }
}

/// (modified time, size, inode): the inode changes on every atomic save, so an edit made within the file
/// system's timestamp granularity of the last one is still noticed.
type Stamp = (SystemTime, u64, u64);

/// The settings plus what is needed to notice that the file changed.
pub struct SettingsStore {
    dir: PathBuf,
    stamp: Option<Stamp>,
    settings: Settings,
    corrupt: bool,
}

impl SettingsStore {
    pub fn open(dir: &Path) -> Self {
        let mut s = Self { dir: dir.to_path_buf(), stamp: None, settings: Settings::default(), corrupt: false };
        s.reload();
        s
    }

    pub fn get(&self) -> Settings {
        self.settings.clone()
    }

    /// The file exists but couldn't be understood, so `get` is the defaults.
    pub fn is_corrupt(&self) -> bool {
        self.corrupt
    }

    pub fn path(&self) -> PathBuf {
        path_in(&self.dir)
    }

    /// Save, then read the file back either way, so what `get` says is what is on disk (after a failed
    /// save that is the old value) and the change counts as seen: nothing waits for a polling interval.
    pub fn update(&mut self, settings: &Settings) -> io::Result<()> {
        let saved = save(&self.dir, settings);
        self.reload();
        saved
    }

    fn stamp(&self) -> Option<Stamp> {
        fs::metadata(path_in(&self.dir)).ok().and_then(|m| Some((m.modified().ok()?, m.len(), m.ino())))
    }

    fn reload(&mut self) {
        self.stamp = self.stamp();
        (self.settings, self.corrupt) = load(&self.dir);
    }

    /// Pick up an edit. One `stat` when nothing changed. True when the file was re-read.
    pub fn refresh(&mut self) -> bool {
        if self.stamp() == self.stamp {
            return false;
        }
        self.reload();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-settings-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        d
    }

    /// The entries of the tab called `title`.
    fn tab(title: &str) -> &'static [Item] {
        &TABS.iter().find(|t| t.title == title).unwrap_or_else(|| panic!("no tab called {title}")).items
    }

    /// The General tab's entries below its Programs (the two program switches come first).
    fn general() -> &'static [Item] {
        &tab("General")[2..]
    }

    fn write(d: &Path, text: &str) {
        fs::create_dir_all(d).unwrap();
        fs::write(path_in(d), text).unwrap();
    }

    #[test]
    fn a_missing_file_gives_the_defaults_and_creates_nothing() {
        let d = dir("missing");
        assert_eq!(load(&d), (Settings { idle_stop_minutes: 120, auto_archive_days: 2, ..Settings::default() }, false));
        assert!(!d.exists(), "reading must not create the directory or the file");
    }

    #[test]
    fn values_are_read_and_a_missing_key_keeps_its_default() {
        let d = dir("read");
        write(&d, r#"{"idle_stop_minutes": 30, "auto_archive_days": 7}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: 30, auto_archive_days: 7, ..Settings::default() });
        write(&d, r#"{"auto_archive_days": 5}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: 120, auto_archive_days: 5, ..Settings::default() });
        write(&d, "{}");
        assert_eq!(load(&d), (Settings::default(), false), "an empty object is a valid file, not a corrupt one");
    }

    #[test]
    fn zero_means_off() {
        let d = dir("zero");
        write(&d, r#"{"idle_stop_minutes": 0, "auto_archive_days": 0}"#);
        let s = load(&d).0;
        assert_eq!((s.idle_stop_minutes, s.auto_archive_days), (0, 0));
        assert_eq!((s.idle_stop_after(60), s.auto_archive_secs()), (None, None));
        let on = Settings::default();
        assert_eq!((on.idle_stop_after(60), on.auto_archive_secs()), (Some(Duration::from_secs(7200)), Some(172_800)));
    }

    #[test]
    fn silly_values_are_clamped_or_ignored() {
        let d = dir("clamp");
        write(&d, r#"{"idle_stop_minutes": 99999999999, "auto_archive_days": 1e30}"#);
        assert_eq!(
            load(&d).0,
            Settings { idle_stop_minutes: MAX_IDLE_STOP_MINUTES, auto_archive_days: MAX_AUTO_ARCHIVE_DAYS, ..Settings::default() }
        );
        write(&d, r#"{"idle_stop_minutes": -5, "auto_archive_days": "soon"}"#);
        assert_eq!(load(&d).0, Settings::default(), "negative numbers and text are nonsense, not 'off'");
        write(&d, r#"{"idle_stop_minutes": 45.9, "auto_archive_days": null}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: 45, auto_archive_days: 2, ..Settings::default() });
    }

    #[test]
    fn a_corrupt_file_reads_as_defaults_is_left_alone_and_backed_up_before_a_save() {
        let d = dir("corrupt");
        for bad in ["{ not json", "[1,2]", "42", ""] {
            write(&d, bad);
            assert_eq!(load(&d), (Settings::default(), true), "{bad:?}");
            assert_eq!(fs::read_to_string(path_in(&d)).unwrap(), bad, "reading must never modify it");
        }
        write(&d, "{ not json");
        save(&d, &Settings { idle_stop_minutes: 10, auto_archive_days: 3, ..Settings::default() }).unwrap();
        assert_eq!(fs::read_to_string(d.join("settings.json.bak")).unwrap(), "{ not json");
        assert_eq!(load(&d), (Settings { idle_stop_minutes: 10, auto_archive_days: 3, ..Settings::default() }, false));

        // A healthy file is replaced without leaving a backup behind, and a second corruption keeps the first backup.
        save(&d, &Settings::default()).unwrap();
        assert!(!d.join("settings.json.bak.1").exists());
        write(&d, "[]");
        save(&d, &Settings::default()).unwrap();
        assert_eq!(fs::read_to_string(d.join("settings.json.bak")).unwrap(), "{ not json");
        assert_eq!(fs::read_to_string(d.join("settings.json.bak.1")).unwrap(), "[]");
    }

    #[test]
    fn save_is_atomic_creates_the_directory_and_clamps() {
        let d = dir("save");
        save(&d, &Settings { idle_stop_minutes: u64::MAX, auto_archive_days: 0, ..Settings::default() }).unwrap();
        assert!(!d.join("settings.json.tmp").exists(), "the temp file is renamed away");
        assert!(!d.join("settings.json.bak").exists(), "nothing to back up on a first save");
        assert_eq!(load(&d), (Settings { idle_stop_minutes: MAX_IDLE_STOP_MINUTES, auto_archive_days: 0, ..Settings::default() }, false));
        let v: Value = serde_json::from_str(&fs::read_to_string(path_in(&d)).unwrap()).unwrap();
        assert_eq!(v["version"], 1);
    }

    #[test]
    fn the_store_notices_an_edit_without_a_restart() {
        let d = dir("store");
        let mut s = SettingsStore::open(&d);
        assert_eq!(s.get(), Settings::default());
        assert!(!s.refresh(), "nothing changed");
        save(&d, &Settings { idle_stop_minutes: 0, auto_archive_days: 9, ..Settings::default() }).unwrap();
        assert!(s.refresh());
        assert_eq!(s.get(), Settings { idle_stop_minutes: 0, auto_archive_days: 9, ..Settings::default() });
        assert!(!s.refresh());
        write(&d, "garbage");
        assert!(s.refresh() && s.is_corrupt());
        assert_eq!(s.get(), Settings::default());
    }

    #[test]
    fn the_editor_settings_default_to_none_chosen_without_asking_and_read_back_what_was_saved() {
        let d = dir("editors");
        let s = load(&d).0;
        assert_eq!((s.notes_editor.as_str(), s.files_editor.as_str(), s.files_editor_ask), (crate::editors::ASK, "", false));
        save(&d, &Settings { notes_editor: "nano".into(), files_editor: "code".into(), files_editor_ask: true, ..Settings::default() })
            .unwrap();
        let back = load(&d).0;
        assert_eq!((back.notes_editor.as_str(), back.files_editor.as_str(), back.files_editor_ask), ("nano", "code", true));
        let v: Value = serde_json::from_str(&fs::read_to_string(path_in(&d)).unwrap()).unwrap();
        assert_eq!((v["files_editor"].as_str(), v["files_editor_ask"].as_bool()), (Some("code"), Some(true)));
        // An editor this version doesn't know (or one uninstalled since) is kept as the file says; nonsense is the default.
        write(&d, r#"{"files_editor":"some-new-ide","files_editor_ask":"yes","mine":1}"#);
        let odd = load(&d).0;
        assert_eq!((odd.files_editor.as_str(), odd.files_editor_ask), ("some-new-ide", false));
        write(&d, r#"{"files_editor":"  ","mine":1}"#);
        assert_eq!(load(&d).0.files_editor, "");
        // An older platoon's own editor, gone: none chosen.
        write(&d, r#"{"files_editor":"platoon","mine":1}"#);
        assert_eq!(load(&d).0.files_editor, "");
        save(&d, &Settings { files_editor_ask: true, ..Settings::default() }).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(path_in(&d)).unwrap()).unwrap();
        assert_eq!(v["mine"], 1, "keys it doesn't know are kept");
    }

    #[test]
    fn the_editors_tab_has_its_two_groups() {
        let tab = &TABS[TAB_EDITORS];
        assert_eq!(tab.title, "Editors");
        let names: Vec<&str> = tab.items.iter().map(|i| i.name).collect();
        assert_eq!(names, ["Editor for notes and tasks (e)", "Editor for folders (v)", "Always ask which editor"]);
        assert_eq!(tab.heading(0), Some("Editing platoon's text"));
        assert_eq!(tab.heading(1), Some("Editing files"));
        assert_eq!(tab.heading(2), None, "the switch belongs with the files editor");
        assert_eq!(tab.items[0].kind, Kind::Editor(EditorFor::Text));
        assert_eq!(tab.items[1].kind, Kind::Editor(EditorFor::Files));
        let ask = &tab.items[2];
        let mut s = Settings::default();
        assert_eq!((ask.label)(ask.value(&s)), "Off");
        ask.assign(&mut s, 1);
        assert!(s.files_editor_ask);
        assert!(
            TABS.iter().enumerate().all(|(i, t)| i == TAB_EDITORS || i == 0 || t.headings.is_empty()),
            "only this tab and General have groups"
        );
    }

    #[test]
    fn save_keeps_every_key_it_does_not_know_including_nested_values_and_their_order() {
        let d = dir("keep-keys");
        write(&d, r#"{"note":"mine","idle_stop_minutes":5,"nested":{"a":[1,{"b":null}],"c":"d"},"version":7}"#);
        save(&d, &Settings { idle_stop_minutes: 60, auto_archive_days: 9, ..Settings::default() }).unwrap();
        let text = fs::read_to_string(path_in(&d)).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["note"], "mine");
        assert_eq!(v["nested"], json!({"a":[1,{"b":null}],"c":"d"}));
        assert_eq!(v["version"], 7, "a version written by something else is not rewritten");
        assert_eq!((v["idle_stop_minutes"].as_u64(), v["auto_archive_days"].as_u64()), (Some(60), Some(9)));
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "note",
                "idle_stop_minutes",
                "nested",
                "version",
                "auto_archive_days",
                "idle_stop_ignores_mcp",
                "search_prompt_chars",
                "search_snippet_chars",
                "search_results",
                "search_archived",
                "tokens_enabled",
                "tokens_asked",
                "ai_refresh_messages",
                "ai_batch_size",
                "ai_daily_tokens",
                "ai_pause_percent",
                "ai_prompts_per_session",
                "ai_prompt_chars",
                "ai_quiet_seconds",
                "ai_archived",
                "ai_archived_share",
                "notes_editor",
                "files_editor",
                "files_editor_ask",
                "notes_quiet_minutes",
                "notes_reply_chars",
                "ai_find_candidates",
                "ai_find_results",
                "ai_find_wide_candidates",
                "update_check",
                "use_claude_code",
                "use_codex",
                "token_features",
                "theme"
            ],
            "the file keeps its shape"
        );
        assert!(!d.join("settings.json.bak").exists(), "a healthy file needs no backup");
    }

    #[test]
    fn a_failed_save_leaves_the_file_and_no_temp_file_behind() {
        let d = dir("fail");
        write(&d, r#"{"idle_stop_minutes":45}"#);
        // The rename target is a directory, so the write succeeds and the rename fails.
        fs::remove_file(path_in(&d)).unwrap();
        fs::create_dir(path_in(&d)).unwrap();
        assert!(save(&d, &Settings::default()).is_err());
        assert!(!d.join("settings.json.tmp").exists());
    }

    #[test]
    fn labels_read_in_the_largest_whole_unit() {
        let l: Vec<String> = [0, 1, 30, 60, 90, 120, 1440, 2880, 1500].map(minutes_label).into();
        assert_eq!(l, ["Off", "1 minute", "30 minutes", "1 hour", "90 minutes", "2 hours", "1 day", "2 days", "25 hours"]);
        assert_eq!([0, 1, 2, 90].map(days_label), ["Off", "1 day", "2 days", "90 days"]);
    }

    #[test]
    fn every_setting_offers_ascending_steps_starting_at_off_within_the_limits() {
        for item in all_items().filter(|i| i.kind == Kind::Steps) {
            assert!(item.choices[0] == 0 || item.off.is_none(), "only an item that can be off starts at 0: {}", item.name);
            assert!(item.choices.windows(2).all(|w| w[0] < w[1]), "{}", item.name);
            assert!(!item.explain.is_empty() && item.explain.len() < 300, "{}", item.name);
            let defaults = Settings::default();
            assert!(item.choices.contains(&(item.get)(&defaults)), "the default is one of the steps for {}", item.name);
            let mut s = defaults;
            (item.set)(&mut s, *item.choices.last().unwrap());
            assert_eq!(s.clone().clamped(), s, "the last step is within the limits for {}", item.name);
        }
    }

    #[test]
    fn stepping_goes_to_the_neighbouring_choice_from_a_step_or_from_between_steps() {
        let idle = &general()[1];
        assert_eq!(idle.step(120, Step::Next), 240);
        assert_eq!(idle.step(120, Step::Prev), 60);
        assert_eq!(idle.step(90, Step::Next), 120, "from a hand-edited value, the nearest step above");
        assert_eq!(idle.step(90, Step::Prev), 60, "and below");
        assert_eq!(idle.step(90, Step::Cycle), 120);
        assert_eq!(idle.step(0, Step::Prev), 0, "the ends hold");
        assert_eq!(idle.step(1440, Step::Next), 1440);
        assert_eq!(idle.step(1440, Step::Cycle), 0, "cycling wraps to Off");
        assert_eq!(idle.step(5000, Step::Cycle), 0);
        assert_eq!(idle.step(5000, Step::Prev), 1440);
        assert_eq!(idle.step(10, Step::Prev), 0);
    }

    #[test]
    fn ignoring_mcp_servers_is_on_by_default_and_only_a_real_boolean_changes_it() {
        let d = dir("mcp-flag");
        assert!(Settings::default().idle_stop_ignores_mcp);
        assert!(load(&d).0.idle_stop_ignores_mcp, "no file");
        write(&d, "{}");
        assert!(load(&d).0.idle_stop_ignores_mcp, "key missing");
        write(&d, r#"{"idle_stop_ignores_mcp": false}"#);
        assert_eq!(load(&d), (Settings { idle_stop_ignores_mcp: false, ..Settings::default() }, false));
        write(&d, r#"{"idle_stop_ignores_mcp": true, "idle_stop_minutes": 30}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: 30, ..Settings::default() });
        // Wrong type: not a choice, so the default stays (and the rest of the file is still read).
        for odd in ["0", "1", "\"false\"", "\"no\"", "null", "[]", "{}", "-1", "0.0"] {
            write(&d, &format!(r#"{{"idle_stop_ignores_mcp": {odd}, "auto_archive_days": 9}}"#));
            assert_eq!(load(&d), (Settings { auto_archive_days: 9, ..Settings::default() }, false), "{odd}");
        }
        for bad in ["{ nope", "[true]", ""] {
            write(&d, bad);
            assert_eq!(load(&d), (Settings::default(), true), "{bad:?}");
        }
    }

    #[test]
    fn the_mcp_choice_is_saved_as_a_boolean_and_other_keys_survive() {
        let d = dir("mcp-save");
        write(&d, r#"{"mine":[1,{"x":null}],"idle_stop_ignores_mcp":true}"#);
        save(&d, &Settings { idle_stop_ignores_mcp: false, ..Settings::default() }).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(path_in(&d)).unwrap()).unwrap();
        assert_eq!(v["idle_stop_ignores_mcp"], json!(false), "a JSON boolean, not a number");
        assert_eq!(v["mine"], json!([1, {"x": null}]));
        assert!(!load(&d).0.idle_stop_ignores_mcp);
        let mut s = SettingsStore::open(&d);
        assert!(!s.get().idle_stop_ignores_mcp);
        save(&d, &Settings::default()).unwrap();
        assert!(s.refresh() && s.get().idle_stop_ignores_mcp, "a running host sees the change");
    }

    #[test]
    fn the_two_way_choice_steps_between_its_two_values_and_off_is_its_first() {
        let choice = &general()[3];
        assert_eq!(choice.name, "Check for a newer platoon");
        assert_eq!(choice.choices, [0, 1]);
        let mut s = Settings::default();
        assert_eq!((choice.get)(&s), 1);
        assert_eq!((choice.label)(1), "Once a day");
        assert_eq!((choice.label)(0), "Off");
        assert_eq!(choice.step(1, Step::Prev), 0);
        assert_eq!(choice.step(0, Step::Prev), 0);
        assert_eq!(choice.step(0, Step::Next), 1);
        assert_eq!(choice.step(1, Step::Next), 1, "the ends hold");
        assert_eq!(choice.step(1, Step::Cycle), 0, "cycling wraps");
        assert_eq!(choice.step(0, Step::Cycle), 1);
        (choice.set)(&mut s, 0);
        assert!(!s.update_check);
        (choice.set)(&mut s, 1);
        assert!(s.update_check);
        assert_eq!(choice.off, Some(0));
        assert!(general()[1..3].iter().all(|i| i.off == Some(0)));
        assert_eq!(find_item("Idle stop and MCP servers"), None, "the MCP choice is not on the screen any more");
    }

    #[test]
    fn update_saves_and_shows_what_is_really_on_disk_even_when_the_save_fails() {
        let d = dir("update");
        let mut store = SettingsStore::open(&d);
        store.update(&Settings { idle_stop_minutes: 30, auto_archive_days: 7, ..Settings::default() }).unwrap();
        assert_eq!(store.get(), Settings { idle_stop_minutes: 30, auto_archive_days: 7, ..Settings::default() });
        assert!(!store.refresh(), "our own save is already seen");
        fs::remove_file(path_in(&d)).unwrap();
        fs::create_dir(path_in(&d)).unwrap(); // saving now fails
        assert!(store.update(&Settings { idle_stop_minutes: 240, auto_archive_days: 7, ..Settings::default() }).is_err());
        assert_eq!(store.get().idle_stop_minutes, 120, "the store says what is in effect, not what was asked for");
    }

    fn saved(d: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path_in(d)).unwrap()).unwrap()
    }

    #[test]
    fn the_theme_defaults_to_catppuccin_mocha_and_only_a_string_changes_it() {
        let d = dir("theme-read");
        assert_eq!(Settings::default().theme, "catppuccin-mocha");
        assert_eq!(load(&d).0.theme, "catppuccin-mocha", "no file");
        write(&d, "{}");
        assert_eq!(load(&d).0.theme, "catppuccin-mocha", "key missing");
        write(&d, r#"{"theme": "nord", "auto_archive_days": 9}"#);
        assert_eq!(load(&d), (Settings { theme: "nord".into(), auto_archive_days: 9, ..Settings::default() }, false));
        // Wrong type: not a choice, so the default stays, and the rest of the file is still read.
        for odd in ["1", "true", "null", "[]", r#"{"name":"latte"}"#, "1.5"] {
            write(&d, &format!(r#"{{"theme": {odd}, "auto_archive_days": 9}}"#));
            assert_eq!(load(&d), (Settings { auto_archive_days: 9, ..Settings::default() }, false), "{odd}");
        }
    }

    #[test]
    fn an_unknown_theme_shows_as_the_default_but_is_kept_as_it_was_written() {
        let d = dir("theme-unknown");
        write(&d, r#"{"theme": "my-own-theme", "idle_stop_minutes": 30}"#);
        let s = load(&d).0;
        assert_eq!(s.theme, "my-own-theme", "the id is carried as written");
        assert_eq!(s.theme(&Registry::default()).id, "catppuccin-mocha", "and displays as the default");
        // Saving something else (what the settings screen does) keeps it.
        save(&d, &Settings { idle_stop_minutes: 60, ..s.clone() }).unwrap();
        assert_eq!(saved(&d)["theme"], "my-own-theme");
        assert_eq!(saved(&d)["idle_stop_minutes"], 60);
        // Picking another one replaces it.
        save(&d, &Settings { theme: "tokyo-night".into(), ..s }).unwrap();
        assert_eq!(saved(&d)["theme"], "tokyo-night");
        assert_eq!(load(&d).0.theme(&Registry::default()).id, "tokyo-night");
    }

    #[test]
    fn a_theme_value_that_is_not_text_survives_saves_until_a_theme_is_picked() {
        let d = dir("theme-object");
        write(&d, r#"{"theme": {"name": "latte", "tweaks": [1]}}"#);
        let s = load(&d).0;
        assert_eq!(s.theme, "catppuccin-mocha");
        save(&d, &Settings { auto_archive_days: 7, ..s.clone() }).unwrap();
        assert_eq!(saved(&d)["theme"], json!({"name": "latte", "tweaks": [1]}), "not ours to overwrite");
        save(&d, &Settings { theme: "dracula".into(), ..s }).unwrap();
        assert_eq!(saved(&d)["theme"], "dracula");
    }

    #[test]
    fn a_first_save_writes_the_theme_and_picking_the_default_over_another_writes_it_back() {
        let d = dir("theme-save");
        save(&d, &Settings::default()).unwrap();
        assert_eq!(saved(&d)["theme"], "catppuccin-mocha");
        save(&d, &Settings { theme: "nord".into(), ..Settings::default() }).unwrap();
        assert_eq!(saved(&d)["theme"], "nord");
        save(&d, &Settings::default()).unwrap();
        assert_eq!(saved(&d)["theme"], "catppuccin-mocha", "choosing the default is a choice like any other");
        // A file without the key gets it when something else changes, like the other settings.
        write(&d, r#"{"idle_stop_minutes": 5}"#);
        save(&d, &Settings::default()).unwrap();
        assert_eq!(saved(&d)["theme"], "catppuccin-mocha");
    }

    #[test]
    fn the_store_notices_a_theme_edit() {
        let d = dir("theme-store");
        write(&d, r#"{"theme": "nord"}"#);
        let mut s = SettingsStore::open(&d);
        assert_eq!(s.get().theme(&Registry::default()).id, "nord");
        write(&d, r#"{"theme": "vesper", "note": "a longer file, so the size changes too"}"#);
        assert!(s.refresh());
        assert_eq!(s.get().theme(&Registry::default()).id, "vesper");
        assert!(!s.refresh());
    }

    #[test]
    fn the_theme_entry_is_first_and_a_kind_of_its_own_next_to_the_stepped_ones() {
        let items = general();
        assert_eq!(items[0].name, "Theme");
        assert_eq!(items[0].kind, Kind::Theme);
        assert!(items[1..].iter().all(|i| i.kind == Kind::Steps), "after it only stepped entries");
        assert!(!all_items().any(|i| i.name == "Edit notes with"), "the old name of the notes editor's setting is gone");
        assert!(
            all_items().filter(|i| i.name != "Theme").all(|i| matches!(
                i.kind,
                Kind::Steps | Kind::Feature(_) | Kind::Instructions(_) | Kind::Editor(_) | Kind::Model(_) | Kind::Program(_)
            )),
            "the theme is the only one of its kind"
        );
        assert!(!items[0].explain.is_empty() && items[0].explain.len() < 300);
        // Stepping the numbers is by choices; the theme has none, so it never moves by them.
        assert_eq!((items[0].step(0, Step::Next), items[0].step(0, Step::Prev), items[0].step(0, Step::Cycle)), (0, 0, 0));
    }

    #[test]
    fn six_tabs_hold_every_setting_once_each_where_it_belongs() {
        let titles: Vec<&str> = TABS.iter().map(|t| t.title).collect();
        assert_eq!(titles, ["General", "Search", "Smart features", "Sessions", "Projects", "Editors"]);
        assert_eq!(
            tab("Editors").iter().map(|i| i.name).collect::<Vec<_>>(),
            ["Editor for notes and tasks (e)", "Editor for folders (v)", "Always ask which editor"]
        );
        assert_eq!((TABS[TAB_SMART].title, TABS[TAB_SESSIONS].title), ("Smart features", "Sessions"));
        let names = |title: &str| tab(title).iter().map(|i| i.name).collect::<Vec<_>>();
        assert_eq!(
            names("General"),
            ["Use Claude Code", "Use Codex", "Theme", "Stop idle sessions after", "Archive sessions after", "Check for a newer platoon"]
        );
        assert_eq!(
            names("Search"),
            [
                "Search archived",
                "Search results shown",
                "Search preview length",
                "Longest message kept",
                "Description search: sessions sent",
                "Description search: results",
                "Description search: wider search",
                "System prompt for Describe it"
            ]
        );
        assert_eq!(
            names("Smart features"),
            [
                MASTER_LABEL,
                "Smart names and summaries",
                "Find a session by description",
                "Project notes",
                "Project descriptions",
                "Daily token limit",
                "Pause above plan usage",
                "Model for smart features"
            ]
        );
        assert_eq!(
            names("Sessions"),
            [
                "Sessions per call",
                "Refresh a summary after",
                "Wait for quiet before refreshing",
                "Recent messages sent",
                "Longest message sent",
                "Archived sessions",
                "Share for archived",
                "System prompt for naming"
            ]
        );
        assert_eq!(
            names("Projects"),
            [
                "Update notes after",
                "Read Claude's last reply up to",
                "Model for project notes",
                "System prompt for project notes",
                "System prompt for project descriptions",
                "System prompt for the hand-off",
                "System prompt for tasks from a conversation"
            ]
        );
        // Every AI action's instructions are an entry, on the tab it belongs to, and nothing else is of that kind.
        let kinds: Vec<(usize, Kind)> = TABS
            .iter()
            .enumerate()
            .flat_map(|(t, tab)| tab.items.iter().filter(|i| matches!(i.kind, Kind::Instructions(_))).map(move |i| (t, i.kind)))
            .collect();
        assert_eq!(
            kinds,
            [
                (1, Kind::Instructions("describe-it")),
                (TAB_SESSIONS, Kind::Instructions("names")),
                (4, Kind::Instructions("project-notes")),
                (4, Kind::Instructions("project-description")),
                (4, Kind::Instructions("hand-off")),
                (4, Kind::Instructions("tasks-from-conversation"))
            ]
        );
        for item in all_items().filter(|i| matches!(i.kind, Kind::Instructions(_))) {
            assert!(item.choices.is_empty() && item.name.starts_with("System prompt for ") && item.explain.len() < 300, "{}", item.name);
            // The line under the entry only says what the prompt is for: the footer shows the keys.
            let lower = item.explain.to_lowercase();
            for key_help in ["enter", "press", " c ", " r ", "(h)", "editor", "kept by platoon", "built-in", "instructions", "restore"] {
                assert!(!lower.contains(key_help), "{}: {:?} in {:?}", item.name, key_help, item.explain);
            }
            assert!(
                item.explain.starts_with("What platoon asks Claude for") || item.explain.starts_with("How platoon asks Claude to"),
                "{}",
                item.explain
            );
        }
        // No entry is on two tabs, and every one can be found by name.
        let all: Vec<&str> = all_items().map(|i| i.name).collect();
        let unique: std::collections::BTreeSet<&str> = all.iter().copied().collect();
        assert_eq!(all.len(), unique.len());
        assert_eq!(find_item("Share for archived"), Some((TAB_SESSIONS, 6)));
        assert_eq!(find_item("Offer tasks before starting work"), None, "the offer is the /to-tickets skill now");
        assert_eq!(find_item("no such thing"), None);
    }

    #[test]
    fn a_user_theme_id_is_found_through_the_registry_and_kept_when_its_file_is_gone() {
        let d = dir("theme-user");
        fs::create_dir_all(d.join("themes")).unwrap();
        fs::write(d.join("themes/Deep Sea"), "background = #0a1622\nforeground = #dde6ee\n").unwrap();
        write(&d, r#"{"theme": "user:deep-sea", "idle_stop_minutes": 30}"#);
        let (s, corrupt) = load(&d);
        assert!(!corrupt);
        assert_eq!(s.theme, "user:deep-sea", "the id is carried as written");
        let reg = Registry::open(&d);
        assert_eq!(s.theme(&reg).name, "Deep Sea");
        assert_eq!(s.theme(&Registry::default()).id, "catppuccin-mocha", "without the file it shows the default");
        // Saving something else keeps the id, whether or not the file is there; choosing another replaces it.
        save(&d, &Settings { idle_stop_minutes: 60, ..s.clone() }).unwrap();
        assert_eq!(saved(&d)["theme"], "user:deep-sea");
        fs::remove_file(d.join("themes/Deep Sea")).unwrap();
        save(&d, &Settings { idle_stop_minutes: 90, theme: Settings::default().theme, ..s.clone() }).unwrap();
        assert_eq!(saved(&d)["theme"], "catppuccin-mocha", "picking the default is a choice like any other");
        save(&d, &Settings { theme: "user:deep-sea".into(), ..s }).unwrap();
        assert_eq!(saved(&d)["theme"], "user:deep-sea");
    }

    #[test]
    fn the_search_settings_default_are_read_clamped_and_kept_with_the_rest_of_the_file() {
        let d = dir("search");
        let def = Settings::default();
        assert_eq!((def.search_prompt_chars, def.search_snippet_chars, def.search_results, def.search_archived), (2000, 120, 50, true));
        assert_eq!(load(&d).0, def, "no file");
        write(&d, r#"{"search_prompt_chars": 5000, "search_snippet_chars": 80, "search_results": 100, "search_archived": false}"#);
        let s = load(&d).0;
        assert_eq!((s.search_prompt_chars, s.search_snippet_chars, s.search_results, s.search_archived), (5000, 80, 100, false));
        // Silly numbers are clamped to what makes sense, not turned into "off".
        write(&d, r#"{"search_prompt_chars": 1, "search_snippet_chars": 0, "search_results": 99999999}"#);
        let s = load(&d).0;
        assert_eq!(
            (s.search_prompt_chars, s.search_snippet_chars, s.search_results),
            (MIN_PROMPT_CHARS, MIN_SNIPPET_CHARS, MAX_SEARCH_RESULTS)
        );
        write(&d, r#"{"search_prompt_chars": 1e30, "search_snippet_chars": 1e30, "search_results": 0}"#);
        let s = load(&d).0;
        assert_eq!(
            (s.search_prompt_chars, s.search_snippet_chars, s.search_results),
            (MAX_PROMPT_CHARS, MAX_SNIPPET_CHARS, MIN_SEARCH_RESULTS)
        );
        // Wrong types keep the default, and the rest of the file is still read.
        for odd in ["\"lots\"", "-5", "null", "[]", "true"] {
            write(
                &d,
                &format!(r#"{{"search_prompt_chars": {odd}, "search_results": {odd}, "search_archived": {odd}, "auto_archive_days": 9}}"#),
            );
            let s = load(&d).0;
            assert_eq!((s.search_prompt_chars, s.search_results, s.auto_archive_days), (2000, 50, 9), "{odd}");
            assert!(s.search_archived, "{odd}: only a real true or false is a choice, and the default is true");
        }
        write(&d, r#"{"search_archived": "no"}"#);
        assert!(load(&d).0.search_archived);
        // Saved, clamped, and the file's other keys survive.
        write(&d, r#"{"mine": [1, 2]}"#);
        save(&d, &Settings { search_prompt_chars: 1, search_results: 10_000, search_archived: false, ..Settings::default() }).unwrap();
        let v = saved(&d);
        assert_eq!(
            (v["search_prompt_chars"].as_u64(), v["search_results"].as_u64(), v["search_archived"].as_bool()),
            (Some(MIN_PROMPT_CHARS), Some(MAX_SEARCH_RESULTS), Some(false))
        );
        assert_eq!(v["mine"], json!([1, 2]));
    }

    #[test]
    fn every_search_entry_is_listed_with_plain_words_and_its_default_among_the_steps() {
        let items = tab("Search");
        let names: Vec<&str> = items.iter().map(|i| i.name).collect();
        assert_eq!(names[0..4], ["Search archived", "Search results shown", "Search preview length", "Longest message kept"]);
        let d = Settings::default();
        assert_eq!((items[0].label)((items[0].get)(&d)), "Include archived");
        assert_eq!((items[1].label)((items[1].get)(&d)), "50 results");
        assert_eq!((items[2].label)((items[2].get)(&d)), "120 characters");
        assert_eq!((items[3].label)((items[3].get)(&d)), "2000 characters");
        assert_eq!((results_label(1), chars_label(1)), ("1 result".to_string(), "1 character".to_string()));
        // (The word search's own entries: the description search's may speak of tokens, it spends them.)
        for item in &items[0..4] {
            let lower = item.explain.to_lowercase();
            for jargon in ["index", "json", "cache", "byte", "token", "thread"] {
                assert!(!lower.contains(jargon), "{}: {jargon}", item.name);
            }
            // Every step is inside the limits the file is clamped to.
            for &c in item.choices {
                let mut s = Settings::default();
                (item.set)(&mut s, c);
                assert_eq!(s.clone().clamped(), s, "{}: step {c}", item.name);
            }
        }
    }

    #[test]
    fn the_token_settings_default_to_off_with_a_daily_limit_that_is_never_unlimited() {
        let d = Settings::default();
        assert!(!d.tokens_enabled && d.token_features.is_empty() && !d.tokens_asked);
        assert_eq!((d.ai_refresh_messages, d.ai_batch_size, d.ai_daily_tokens, d.ai_pause_percent), (5, 30, 500_000, 80));
        // No model picked: each follows what this computer has (where Claude Code is: Haiku, and Sonnet for the notes).
        assert_eq!((d.ai_model.as_deref(), d.notes_model.as_deref()), (None, None));
        let claude = crate::agents::Providers { claude: Some("claude-stand-in".into()), codex: None, off: Vec::new() }.catalog();
        assert_eq!((d.smart_model_in(&claude).as_deref(), d.notes_model_in(&claude).as_deref()), (Some("haiku"), Some("sonnet")));
        for f in FEATURES {
            // The master switch is off, so nothing may spend anything; a feature added later is on once it is on.
            assert!(!d.feature_on(f.id), "{}", f.id);
            assert_eq!(d.feature_toggle(f.id), f.on_unless_switched_off, "{}", f.id);
        }
        let dir = dir("tokens-default");
        assert_eq!(load(&dir).0, d, "no file");
        write(&dir, "{}");
        assert_eq!(load(&dir).0, d);
    }

    #[test]
    fn the_token_settings_are_read_clamped_and_wrong_types_keep_the_default() {
        let d = dir("tokens-read");
        write(
            &d,
            r#"{"tokens_enabled":true,"tokens_asked":true,"token_features":{"names":true,"find":false},"ai_refresh_messages":10,"ai_batch_size":50,"ai_daily_tokens":1000000,"ai_pause_percent":90,"ai_model":"sonnet"}"#,
        );
        let s = load(&d).0;
        assert!(s.tokens_enabled && s.tokens_asked && s.feature_on("names") && !s.feature_on("find"));
        assert_eq!(
            (s.ai_refresh_messages, s.ai_batch_size, s.ai_daily_tokens, s.ai_pause_percent, s.ai_model.as_deref()),
            (10, 50, 1_000_000, 90, Some("sonnet"))
        );
        // Silly numbers are clamped: never zero messages or conversations, and never an unlimited day.
        write(&d, r#"{"ai_refresh_messages":0,"ai_batch_size":0,"ai_daily_tokens":0,"ai_pause_percent":3}"#);
        let s = load(&d).0;
        assert_eq!(
            (s.ai_refresh_messages, s.ai_batch_size, s.ai_daily_tokens, s.ai_pause_percent),
            (MIN_REFRESH_MESSAGES, MIN_BATCH, MIN_DAILY_TOKENS, MIN_PAUSE_PERCENT)
        );
        write(&d, r#"{"ai_refresh_messages":1e30,"ai_batch_size":99999,"ai_daily_tokens":1e30,"ai_pause_percent":500}"#);
        let s = load(&d).0;
        assert_eq!(
            (s.ai_refresh_messages, s.ai_batch_size, s.ai_daily_tokens, s.ai_pause_percent),
            (MAX_REFRESH_MESSAGES, MAX_BATCH, MAX_DAILY_TOKENS, MAX_PAUSE_PERCENT)
        );
        // 0 for the pause point means "never pause" and stays 0.
        write(&d, r#"{"ai_pause_percent":0}"#);
        assert_eq!(load(&d).0.ai_pause_percent, 0);
        // A model is kept exactly as written, one this computer doesn't have too (it is said, never swapped for
        // another); wrong types keep every default.
        write(&d, r#"{"ai_model":"gpt-9","notes_model":"opus"}"#);
        assert_eq!((load(&d).0.ai_model.as_deref(), load(&d).0.notes_model.as_deref()), (Some("gpt-9"), Some("opus")));
        for odd in ["\"yes\"", "1", "null", "[]", "{}"] {
            write(
                &d,
                &format!(
                    r#"{{"tokens_enabled":{odd},"tokens_asked":{odd},"token_features":{odd},"ai_model":{odd},"ai_batch_size":{odd},"auto_archive_days":9}}"#
                ),
            );
            let s = load(&d).0;
            // Text is a model's name, kept as written; anything else is no model picked.
            let model = (odd == "\"yes\"").then_some("yes");
            assert_eq!(
                (s.tokens_enabled, s.tokens_asked, s.token_features.len(), s.ai_model.as_deref(), s.auto_archive_days),
                (false, false, 0, model, 9),
                "{odd}"
            );
        }
        write(&d, r#"{"token_features":{"names":"yes","find":true,"future":true}}"#);
        let s = load(&d).0;
        assert_eq!(
            s.token_features.into_iter().collect::<Vec<_>>(),
            [("find".to_string(), true), ("future".to_string(), true)],
            "only real booleans count"
        );
    }

    #[test]
    fn the_master_switch_off_turns_every_feature_off_whatever_their_own_switches_say() {
        let mut s = Settings { tokens_enabled: true, ..Settings::default() };
        for f in FEATURES {
            s.set_feature(f.id, true);
            assert!(s.feature_on(f.id));
        }
        s.tokens_enabled = false;
        for f in FEATURES {
            assert!(!s.feature_on(f.id), "{}", f.id);
        }
        // ...through a save and a load, too.
        let d = dir("tokens-master");
        save(&d, &s).unwrap();
        let back = load(&d).0;
        assert!(FEATURES.iter().all(|f| !back.feature_on(f.id) && back.feature_toggle(f.id)));
        // A feature that was never mentioned is off even with the master on.
        assert!(!Settings { tokens_enabled: true, ..Settings::default() }.feature_on("names"));
    }

    #[test]
    fn saving_the_token_settings_keeps_unknown_features_and_keys_and_writes_real_booleans() {
        let d = dir("tokens-save");
        write(&d, r#"{"mine":[1],"token_features":{"future":true,"names":false,"note":"x"}}"#);
        let mut s = load(&d).0;
        assert_eq!(s.token_features.get("future"), Some(&true));
        s.tokens_enabled = true;
        s.set_feature("names", true);
        s.ai_model = Some("sonnet".into());
        save(&d, &s).unwrap();
        let v = saved(&d);
        assert_eq!(
            v["token_features"],
            json!({"future": true, "names": true, "note": "x"}),
            "a newer version's feature and a non-boolean entry stay"
        );
        assert_eq!((v["tokens_enabled"].clone(), v["ai_model"].clone(), v["mine"].clone()), (json!(true), json!("sonnet"), json!([1])));
        // Clamped on the way out.
        save(&d, &Settings { ai_daily_tokens: 5, ai_batch_size: 0, ..s }).unwrap();
        let v = saved(&d);
        assert_eq!((v["ai_daily_tokens"].as_u64(), v["ai_batch_size"].as_u64()), (Some(MIN_DAILY_TOKENS), Some(MIN_BATCH)));
    }

    #[test]
    fn both_programs_are_on_unless_switched_off_and_only_a_real_boolean_switches_one() {
        let d = dir("programs");
        let s = load(&d).0;
        assert!(s.use_claude_code && s.use_codex && s.switched_off().is_empty(), "on by default");
        write(&d, r#"{"use_codex": false, "mine": [1]}"#);
        let s = load(&d).0;
        assert_eq!((s.use_claude_code, s.use_codex, s.switched_off()), (true, false, vec![Agent::Codex]));
        assert!(s.uses(Agent::Claude) && !s.uses(Agent::Codex));
        let mut both = s.clone();
        both.set_uses(Agent::Claude, false);
        assert_eq!(both.switched_off(), [Agent::Claude, Agent::Codex], "Claude Code first");
        // Saved as JSON booleans, every other key kept.
        save(&d, &Settings { use_codex: true, use_claude_code: false, ..s }).unwrap();
        assert_eq!((saved(&d)["use_claude_code"].clone(), saved(&d)["use_codex"].clone()), (json!(false), json!(true)));
        assert_eq!(saved(&d)["mine"], json!([1]));
        assert_eq!(load(&d).0.switched_off(), [Agent::Claude]);
        // Anything but true or false is no choice: on.
        for odd in ["0", "\"off\"", "null", "[]"] {
            write(&d, &format!(r#"{{"use_claude_code": {odd}, "use_codex": {odd}}}"#));
            assert!(load(&d).0.switched_off().is_empty(), "{odd}");
        }
    }

    #[test]
    fn the_agent_new_sessions_start_in_is_written_only_once_chosen_and_kept_as_written() {
        let d = dir("new-session-agent");
        write(&d, r#"{"mine":[1]}"#);
        let s = load(&d).0;
        assert_eq!(s.new_session_agent, None);
        save(&d, &s).unwrap();
        assert!(saved(&d).get("new_session_agent").is_none(), "nothing chosen yet: nothing written");
        save(&d, &Settings { new_session_agent: Some("codex".into()), ..s }).unwrap();
        assert_eq!((saved(&d)["new_session_agent"].clone(), saved(&d)["mine"].clone()), (json!("codex"), json!([1])));
        assert_eq!(load(&d).0.new_session_agent.as_deref(), Some("codex"));
        // A word this version doesn't know is kept as it is (it reads as no choice: `Providers::new_session_agent`).
        write(&d, r#"{"new_session_agent":"gemini"}"#);
        let s = load(&d).0;
        assert_eq!(s.new_session_agent.as_deref(), Some("gemini"));
        save(&d, &s).unwrap();
        assert_eq!(saved(&d)["new_session_agent"], json!("gemini"));
        write(&d, r#"{"new_session_agent":5}"#);
        assert_eq!(load(&d).0.new_session_agent, None);
    }

    #[test]
    fn a_feature_gets_a_switch_on_the_smart_features_tab_from_the_table_alone() {
        let smart = tab("Smart features");
        let names: Vec<&str> = smart.iter().map(|i| i.name).collect();
        assert_eq!(
            names[..6],
            [
                "Enable features which require tokens",
                "Smart names and summaries",
                "Find a session by description",
                "Project notes",
                "Project descriptions",
                "Daily token limit"
            ]
        );
        assert_eq!(names.len(), 1 + FEATURES.len() + 3, "the master switch, one per feature, and the three limits on the day's use");
        for (f, item) in FEATURES.iter().zip(&smart[1..]) {
            assert_eq!((item.name, item.explain, item.kind), (f.label, f.description, Kind::Feature(f.id)));
            let mut s = Settings::default();
            assert_eq!(item.value(&s), u64::from(f.on_unless_switched_off));
            item.assign(&mut s, 0);
            assert!(!s.feature_toggle(f.id) && item.value(&s) == 0, "{}", f.id);
            item.assign(&mut s, 1);
            assert!(s.feature_toggle(f.id) && item.value(&s) == 1);
            assert!(!s.feature_on(f.id), "its own switch is not enough");
            assert_eq!((item.label)(1), "On");
            assert_eq!(item.step(0, Step::Cycle), 1);
        }
        // The master switch reads Off until it is turned on.
        let master = &smart[0];
        assert_eq!((master.value(&Settings::default()), (master.label)(master.value(&Settings::default()))), (0, "Off".to_string()));
        assert_eq!((master.label)(master.value(&Settings { tokens_enabled: true, ..Settings::default() })), "On");
    }

    #[test]
    fn the_token_entries_read_plainly_and_every_step_is_within_the_limits() {
        // Everything that spends tokens or decides what is sent: the smart features, sessions and projects tabs, and the
        // description search.
        let token_items = || TABS[TAB_SMART..TAB_EDITORS].iter().flat_map(|t| t.items.iter()).chain(tab("Search")[4..].iter());
        for item in token_items() {
            let lower = item.explain.to_lowercase();
            for jargon in ["index", "json", "cache", "byte", "thread", "api", "cli", "stdin", "process"] {
                assert!(!lower.contains(jargon), "{}: {jargon}", item.name);
            }
            assert!(item.explain.len() < 300, "{}", item.name);
            for &c in item.choices {
                let mut s = Settings::default();
                item.assign(&mut s, c);
                assert_eq!(s.clone().clamped(), s, "{}: step {c}", item.name);
            }
            let d = Settings::default();
            assert!(item.choices.is_empty() || item.choices.contains(&item.value(&d)), "the default is one of the steps for {}", item.name);
            assert!(item.choices.windows(2).all(|w| w[0] < w[1]), "{}: steps ascend", item.name);
        }
        let d = Settings::default();
        assert_eq!(tab("Smart features")[0].name, MASTER_LABEL);
        assert_eq!(
            token_items().map(|i| (i.label)(i.value(&d))).collect::<Vec<_>>(),
            [
                "Off",
                "Off",
                "Off",
                "On",
                "On",
                "500K tokens",
                "80%",
                "",
                "30 sessions",
                "5 messages",
                "1 minute",
                "8 messages",
                "300 characters",
                "Skip",
                "50%",
                "",
                "3 minutes",
                "1500 characters",
                "",
                "",
                "",
                "",
                "",
                "50 sessions",
                "10 results",
                "200 sessions",
                ""
            ]
        );
        let by_name = |name: &str| all_items().find(|i| i.name == name).unwrap_or_else(|| panic!("no entry called {name}"));
        assert_eq!(by_name("Pause above plan usage").step(0, Step::Cycle), 50, "Never is the first step");
        assert_eq!(percent_label(0), "Never");
        assert_eq!(
            [tokens_label(1_000_000), tokens_label(250_000), tokens_label(1234), tokens_label(1)],
            ["1M tokens", "250K tokens", "1234 tokens", "1 token"]
        );
        assert_eq!([thousands(0), thousands(999), thousands(1000), thousands(1_234_567)], ["0", "999", "1,000", "1,234,567"]);
        assert_eq!(
            [0, 15, 45, 60, 90, 120, 600].map(seconds_label),
            ["No wait", "15 seconds", "45 seconds", "1 minute", "90 seconds", "2 minutes", "10 minutes"]
        );
    }

    #[test]
    fn the_new_numbers_have_defaults_limits_and_survive_a_round_trip() {
        let d = Settings::default();
        assert_eq!(
            (
                d.ai_prompts_per_session,
                d.ai_prompt_chars,
                d.ai_quiet_seconds,
                d.ai_archived,
                d.ai_archived_share,
                d.ai_find_candidates,
                d.ai_find_results,
                d.ai_find_wide_candidates
            ),
            (8, 300, 60, false, 50, 50, 10, 200)
        );
        let dir = dir("new-numbers");
        write(
            &dir,
            r#"{"ai_prompts_per_session":0,"ai_prompt_chars":1,"ai_quiet_seconds":999999,"ai_archived_share":0,"ai_find_candidates":1,"ai_find_results":99,"ai_find_wide_candidates":1}"#,
        );
        let s = load(&dir).0;
        assert_eq!(
            (
                s.ai_prompts_per_session,
                s.ai_prompt_chars,
                s.ai_quiet_seconds,
                s.ai_archived_share,
                s.ai_find_candidates,
                s.ai_find_results,
                s.ai_find_wide_candidates
            ),
            (
                MIN_PROMPTS_PER_SESSION,
                MIN_PROMPT_SEND_CHARS,
                MAX_QUIET_SECONDS,
                MIN_ARCHIVED_SHARE,
                MIN_FIND_CANDIDATES,
                MAX_FIND_RESULTS,
                MIN_FIND_WIDE
            )
        );
        // "ai_archived" is "off" or "background" in the file; a hand-written true or false is understood, anything else is off.
        for (text, want) in
            [(r#""background""#, true), (r#""off""#, false), ("true", true), ("false", false), (r#""sometimes""#, false), ("7", false)]
        {
            write(&dir, &format!(r#"{{"ai_archived":{text}}}"#));
            assert_eq!(load(&dir).0.ai_archived, want, "{text}");
        }
        let s = Settings { ai_archived: true, ai_quiet_seconds: 0, ai_find_results: 7, ..Settings::default() };
        save(&dir, &s).unwrap();
        let v = saved(&dir);
        assert_eq!(
            (v["ai_archived"].clone(), v["ai_quiet_seconds"].clone(), v["ai_find_results"].clone()),
            (json!("background"), json!(0), json!(7))
        );
        assert_eq!(load(&dir).0, s);
        save(&dir, &Settings::default()).unwrap();
        assert_eq!(saved(&dir)["ai_archived"], "off");
    }

    #[test]
    fn the_check_for_a_newer_platoon_is_on_by_default_switched_off_on_general_and_saved() {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-settings-update-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        assert!(Settings::default().update_check);
        let (tab, at) = find_item("Check for a newer platoon").expect("on the settings screen");
        assert_eq!(TABS[tab].title, "General");
        let item = &TABS[tab].items[at];
        assert_eq!(item.choices.iter().map(|&c| (item.label)(c)).collect::<Vec<_>>(), ["Off", "Once a day"]);
        assert_eq!(item.off, Some(0));
        let mut s = Settings::default();
        assert_eq!((item.label)(item.value(&s)), "Once a day");
        item.assign(&mut s, 0);
        assert!(!s.update_check);
        save(&d, &s).unwrap();
        assert!(!load(&d).0.update_check);
        let v: Value = serde_json::from_str(&fs::read_to_string(path_in(&d)).unwrap()).unwrap();
        assert_eq!(v["update_check"], false);
        // Only a real true/false counts.
        fs::write(path_in(&d), r#"{"update_check":"no"}"#).unwrap();
        assert!(load(&d).0.update_check);
    }

    #[test]
    fn an_offer_tasks_key_an_older_platoon_wrote_is_kept_and_harmless() {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-settings-offer-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fs::write(path_in(&d), r#"{"offer_tasks": false, "update_check": false}"#).unwrap();
        let (s, corrupt) = load(&d);
        assert!(!corrupt && !s.update_check);
        save(&d, &s).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(path_in(&d)).unwrap()).unwrap();
        assert_eq!(v["offer_tasks"], false, "a key platoon no longer knows stays as it was");
    }
}
