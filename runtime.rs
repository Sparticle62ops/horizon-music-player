//  Music Player ported to Horizon as a native mod (slice 1).
//  Upstream: github.com/MC-U-Team/Music-Player, commit 1120a0c ("Add new changelog", 2026-10-25),
//  Apache-2.0 (c) 2024 U-Team. Modrinth: https://modrinth.com/mod/music-player (278,370 downloads).
//  Port policy: 1:1 behavior port, no optimizations or behavior changes. Slice 1 covers the core
//  non-GUI logic: client config, settings + managers, playlists (data + playing queue state machine),
//  the track manager over a stand-in audio player, and the skip utility. Audio decode/streaming
//  (lavaplayer, youtube/soundcloud sources) and the GUI are later slices; see README degradations.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const MOD_ID: &str = "music_player";
pub const MOD_NAME: &str = "Music Player";
pub const MOD_VERSION: &str = "1.0.0";

static DEBUG: AtomicBool = AtomicBool::new(false);

pub fn set_debug(debug: bool) {
    DEBUG.store(debug, Ordering::SeqCst);
}

/// Upstream logs via log4j; dev-env-only lines become debug_log.
fn debug_log(message: &str) {
    if DEBUG.load(Ordering::SeqCst) {
        eprintln!("[music_player] {}", message);
    }
}

fn log_error(message: &str) {
    eprintln!("[music_player] ERROR: {}", message);
}

// ---------------------------------------------------------------------------
// util/WrappedObject.java -> WrappedObject
//
// Java uses the default (identity) equals/hashCode, so two wrappers holding the
// same string are NOT equal. The port models that with a unique id compared for
// equality; the serialized form is the upstream gson shape {"o": <string>}
// (@SerializedName(value = "o", alternate = "object")). A deserialized wrapper
// is a fresh object with fresh identity, exactly like gson would produce.
// ---------------------------------------------------------------------------

static WRAPPED_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct WrappedObject {
    object: String,
    id: u64,
}

impl WrappedObject {
    pub fn new(object: String) -> Self {
        WrappedObject {
            object,
            id: WRAPPED_COUNTER.fetch_add(1, Ordering::SeqCst),
        }
    }

    pub fn get(&self) -> &str {
        &self.object
    }
}

impl PartialEq for WrappedObject {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for WrappedObject {}

impl Serialize for WrappedObject {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("o", &self.object)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for WrappedObject {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Helper {
            #[serde(default, rename = "o")]
            o: Option<String>,
            #[serde(default, rename = "object")]
            object: Option<String>,
        }
        let helper = Helper::deserialize(deserializer)?;
        let object = helper
            .o
            .or(helper.object)
            .ok_or_else(|| serde::de::Error::custom("wrapped object has no value"))?;
        Ok(WrappedObject::new(object))
    }
}

// ---------------------------------------------------------------------------
// config/ClientConfig.java -> ClientConfig (musicplayer.json)
//
// One option: internalPlaylists (default false). Missing file -> write the
// default. An IOException on read -> log, then the `config == null` tail writes
// the default. A malformed file is an uncaught JsonSyntaxException upstream, so
// the port panics with the same effect.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    #[serde(rename = "internalPlaylists")]
    pub internal_playlists: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            internal_playlists: false,
        }
    }
}

impl ClientConfig {
    pub fn load(path: &Path) -> ClientConfig {
        if !path.exists() {
            let config = ClientConfig::default();
            config.write(path);
            return config;
        }
        match std::fs::read_to_string(path) {
            Ok(text) => {
                // Uncaught JsonSyntaxException upstream: malformed JSON crashes the client.
                let config: ClientConfig = serde_json::from_str(&text).expect("malformed musicplayer.json");
                config
            }
            Err(ex) => {
                log_error(&format!("Could not read musicplayer config file at {}: {}", path.display(), ex));
                let config = ClientConfig::default();
                config.write(path);
                config
            }
        }
    }

    fn write(&self, path: &Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

// ---------------------------------------------------------------------------
// musicplayer/settings/Repeat.java -> Repeat
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[serde(rename = "NO")]
    No,
    #[serde(rename = "PLAYLIST")]
    Playlist,
    #[serde(rename = "SINGLE")]
    Single,
}

impl Repeat {
    pub fn is_active(self) -> bool {
        match self {
            Repeat::No => false,
            Repeat::Playlist | Repeat::Single => true,
        }
    }

    pub fn forward_cycle(self) -> Self {
        match self {
            Repeat::No => Repeat::Playlist,
            Repeat::Playlist => Repeat::Single,
            Repeat::Single => Repeat::No,
        }
    }
}

// ---------------------------------------------------------------------------
// musicplayer/settings/IngameOverlayPosition.java -> IngameOverlayPosition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IngameOverlayPosition {
    #[serde(rename = "UP_LEFT")]
    UpLeft,
    #[serde(rename = "UP_RIGHT")]
    UpRight,
    #[serde(rename = "DOWN_RIGHT")]
    DownRight,
    #[serde(rename = "DOWN_LEFT")]
    DownLeft,
}

impl IngameOverlayPosition {
    /// Upstream localization keys (MusicPlayerLocalization).
    pub fn localization(self) -> &'static str {
        match self {
            IngameOverlayPosition::UpLeft => "gui.settings.position.up_left",
            IngameOverlayPosition::UpRight => "gui.settings.position.up_right",
            IngameOverlayPosition::DownRight => "gui.settings.position.down_right",
            IngameOverlayPosition::DownLeft => "gui.settings.position.down_left",
        }
    }

    pub fn is_up(self) -> bool {
        matches!(self, IngameOverlayPosition::UpLeft | IngameOverlayPosition::UpRight)
    }

    pub fn is_left(self) -> bool {
        matches!(self, IngameOverlayPosition::UpLeft | IngameOverlayPosition::DownLeft)
    }

    pub fn forward_cycle(self) -> Self {
        match self {
            IngameOverlayPosition::UpLeft => IngameOverlayPosition::UpRight,
            IngameOverlayPosition::UpRight => IngameOverlayPosition::DownRight,
            IngameOverlayPosition::DownRight => IngameOverlayPosition::DownLeft,
            IngameOverlayPosition::DownLeft => IngameOverlayPosition::UpLeft,
        }
    }
}

// ---------------------------------------------------------------------------
// musicplayer/playlist/Skip.java -> Skip
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    Forward,
    Previous,
}

impl Skip {
    pub fn value(self) -> i64 {
        match self {
            Skip::Forward => 1,
            Skip::Previous => -1,
        }
    }
}

// ---------------------------------------------------------------------------
// util/TimeUtil.java -> time_conversion
// ---------------------------------------------------------------------------

const MINUTES_IN_AN_HOUR: i64 = 60;
const SECONDS_IN_A_MINUTE: i64 = 60;

pub fn time_conversion(mut seconds: i64) -> String {
    let mut minutes = seconds / SECONDS_IN_A_MINUTE;
    seconds -= minutes * SECONDS_IN_A_MINUTE;

    let hours = minutes / MINUTES_IN_AN_HOUR;
    minutes -= hours * MINUTES_IN_AN_HOUR;

    format!(
        "{}{:02}:{:02}",
        if hours > 0 { format!("{}:", hours) } else { String::new() },
        minutes,
        seconds
    )
}

// ---------------------------------------------------------------------------
// musicplayer/MusicPlayerFiles.java -> MusicPlayerFiles
//
// internalPlaylists -> <game_dir>/config/musicplayer, otherwise the appdirs
// user config dir for app name "musicplayer" with vendor "mc-u-team" (roaming),
// which on Linux is the XDG config dir: $XDG_CONFIG_HOME/musicplayer or
// ~/.config/musicplayer.
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MusicPlayerFiles {
    directory: Option<PathBuf>,
}

impl MusicPlayerFiles {
    pub fn new() -> Self {
        MusicPlayerFiles::default()
    }

    pub fn load(&mut self, internal_playlists: bool, game_dir: &Path) {
        self.directory = Some(if internal_playlists {
            game_dir.join("config/musicplayer")
        } else {
            user_config_dir("musicplayer")
        });

        if let Err(ex) = std::fs::create_dir_all(self.directory.as_ref().unwrap()) {
            log_error(&format!("Could not create music player directories: {}", ex));
        }
    }

    pub fn get_directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }
}

fn user_config_dir(app: &str) -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join(app);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join(app)
}

// ---------------------------------------------------------------------------
// lavaplayer data model (slice 1 stand-ins).
//
// The audio engine (lavaplayer, youtube/soundcloud sources, PCM output) is a
// later slice and a documented degradation: a Horizon native mod has no audio
// decode/streaming surface yet. The port models the IAudioTrack /
// IAudioTrackList / ISearchResult data so the queue, playlist and manager logic
// can run 1:1 against a pluggable search backend.
// ---------------------------------------------------------------------------

/// IAudioTrackInfo + IAudioTrack (data slice): uri, title, duration, stream
/// flag and the current position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TrackData {
    pub uri: String,
    pub title: String,
    /// Duration in milliseconds; -1 is lavaplayer's "unknown".
    pub duration_ms: i64,
    pub is_stream: bool,
    pub position_ms: i64,
}

impl TrackData {
    pub fn new(uri: &str, title: &str, duration_ms: i64, is_stream: bool) -> Self {
        TrackData {
            uri: uri.to_string(),
            title: title.to_string(),
            duration_ms,
            is_stream,
            position_ms: 0,
        }
    }

    pub fn is_seekable(&self) -> bool {
        !self.is_stream
    }

    pub fn get_uri(&self) -> &str {
        &self.uri
    }

    pub fn get_title(&self) -> &str {
        &self.title
    }

    pub fn get_duration(&self) -> i64 {
        self.duration_ms
    }

    pub fn is_stream(&self) -> bool {
        self.is_stream
    }

    /// The stored track position (the raw IAudioTrack position).
    pub fn get_position(&self) -> i64 {
        self.position_ms
    }

    /// PlayingTrackImpl.getPosition: seekable tracks read the live player
    /// position (clamped to [0, duration]), streams read the track position.
    pub fn position_at(&self, current_track_position: i64) -> i64 {
        if self.is_seekable() {
            self.current_position_clamped(current_track_position)
        } else {
            self.position_ms
        }
    }

    fn current_position_clamped(&self, current_track_position: i64) -> i64 {
        current_track_position.clamp(0, self.duration_ms.max(0))
    }

    /// PlayingTrackImpl.setPosition: only seekable tracks accept a seek, and it
    /// is clamped to [0, duration]. The caller must also update the player's
    /// currentTrackPosition (see TrackManager::set_position).
    pub fn set_position(&mut self, position: i64, current_track_position: &mut i64) {
        if self.is_seekable() {
            let clamped = position.clamp(0, self.duration_ms.max(0));
            self.position_ms = clamped;
            *current_track_position = clamped;
        }
    }
}

/// IAudioTrackList (data slice).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TrackListData {
    pub name: String,
    pub is_search: bool,
    pub uri: Option<String>,
    pub tracks: Vec<TrackData>,
}

impl TrackListData {
    pub fn named(name: &str, tracks: Vec<TrackData>) -> Self {
        TrackListData {
            name: name.to_string(),
            is_search: false,
            uri: None,
            tracks,
        }
    }

    pub fn has_uri(&self) -> bool {
        self.uri.is_some()
    }

    pub fn is_search(&self) -> bool {
        self.is_search
    }

    pub fn get_tracks(&self) -> &[TrackData] {
        &self.tracks
    }
}

/// ISearchResult (data slice).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchResultData {
    pub error_message: Option<String>,
    pub track: Option<TrackData>,
    pub track_list: Option<TrackListData>,
}

impl SearchResultData {
    pub fn errored(message: &str) -> Self {
        SearchResultData {
            error_message: Some(message.to_string()),
            ..Default::default()
        }
    }

    pub fn of_track(track: TrackData) -> Self {
        SearchResultData {
            track: Some(track),
            ..Default::default()
        }
    }

    pub fn of_list(track_list: TrackListData) -> Self {
        SearchResultData {
            track_list: Some(track_list),
            ..Default::default()
        }
    }

    pub fn has_error(&self) -> bool {
        self.error_message.is_some()
    }

    pub fn get_error_message(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    pub fn is_list(&self) -> bool {
        self.track_list.is_some()
    }

    pub fn get_track(&self) -> Option<&TrackData> {
        self.track.as_ref()
    }

    pub fn get_track_list(&self) -> Option<&TrackListData> {
        self.track_list.as_ref()
    }
}

// ---------------------------------------------------------------------------
// musicplayer/playlist/LoadedTracks.java -> LoadedTracks
//
// One playlist uri and what was loaded for it: an error, a single track, or a
// track list. Java compares these by identity, so the port gives each entry a
// unique id and equality is by id only.
// ---------------------------------------------------------------------------

static LOADED_TRACKS_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct LoadedTracks {
    id: u64,
    uri: WrappedObject,
    error_message: Option<String>,
    title: Option<String>,
    track: Option<TrackData>,
    track_list: Option<TrackListData>,
}

impl PartialEq for LoadedTracks {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for LoadedTracks {}

impl LoadedTracks {
    /// Dummy entry used while the uri is still being resolved.
    pub fn dummy(uri: WrappedObject) -> Self {
        LoadedTracks {
            id: LOADED_TRACKS_COUNTER.fetch_add(1, Ordering::SeqCst),
            uri,
            error_message: Some("Not loaded yet".to_string()),
            title: None,
            track: None,
            track_list: None,
        }
    }

    pub fn from_search(uri: WrappedObject, result: SearchResultData) -> Self {
        if result.has_error() {
            LoadedTracks {
                id: LOADED_TRACKS_COUNTER.fetch_add(1, Ordering::SeqCst),
                error_message: result.error_message,
                uri,
                title: None,
                track: None,
                track_list: None,
            }
        } else if !result.is_list() {
            let track = result.track.unwrap();
            let title = track.title.clone();
            LoadedTracks {
                id: LOADED_TRACKS_COUNTER.fetch_add(1, Ordering::SeqCst),
                uri,
                error_message: None,
                title: Some(title),
                track: Some(track),
                track_list: None,
            }
        } else {
            let track_list = result.track_list.unwrap();
            let title = track_list.name.clone();
            LoadedTracks {
                id: LOADED_TRACKS_COUNTER.fetch_add(1, Ordering::SeqCst),
                uri,
                error_message: None,
                title: Some(title),
                track: None,
                track_list: Some(track_list),
            }
        }
    }

    pub fn from_track(uri: WrappedObject, track: TrackData) -> Self {
        let title = track.title.clone();
        LoadedTracks {
            id: LOADED_TRACKS_COUNTER.fetch_add(1, Ordering::SeqCst),
            uri,
            error_message: None,
            title: Some(title),
            track: Some(track),
            track_list: None,
        }
    }

    pub fn from_track_list(uri: WrappedObject, track_list: TrackListData) -> Self {
        let title = track_list.name.clone();
        LoadedTracks {
            id: LOADED_TRACKS_COUNTER.fetch_add(1, Ordering::SeqCst),
            uri,
            error_message: None,
            title: Some(title),
            track: None,
            track_list: Some(track_list),
        }
    }

    pub fn get_uri(&self) -> &WrappedObject {
        &self.uri
    }

    pub fn has_error(&self) -> bool {
        self.error_message.is_some()
    }

    pub fn get_error_message(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    pub fn get_title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn is_track(&self) -> bool {
        self.track_list.is_none() && self.track.is_some()
    }

    pub fn is_track_list(&self) -> bool {
        self.track_list.is_some() && self.track.is_none()
    }

    pub fn get_track(&self) -> Option<&TrackData> {
        self.track.as_ref()
    }

    pub fn get_track_list(&self) -> Option<&TrackListData> {
        self.track_list.as_ref()
    }

    pub fn get_first_track(&self) -> Option<TrackData> {
        if self.is_track() {
            self.track.clone()
        } else if self.is_track_list() {
            self.track_list.as_ref().and_then(|list| list.tracks.first().cloned())
        } else {
            None
        }
    }

    pub fn get_last_track(&self) -> Option<TrackData> {
        if self.is_track() {
            self.track.clone()
        } else if self.is_track_list() {
            self.track_list.as_ref().and_then(|list| list.tracks.last().cloned())
        } else {
            None
        }
    }

    /// getOtherTrack: only a track list can step to a neighbor. `isTrack() ||
    /// !isTrackList()` covers plain tracks, error entries and dummies.
    pub fn get_other_track(&self, track: Option<&TrackData>, skip: Skip) -> Option<TrackData> {
        if self.is_track() || !self.is_track_list() {
            return None;
        }
        let list = self.track_list.as_ref()?;
        // Java's List.indexOf(null) is -1 when absent, and the caller may pass
        // a null track (the side-entry lookup in findNextSong does), so a
        // missing match lands at -1 instead of bailing out: FORWARD then reads
        // index 0, PREVIOUS reads -2 and finds nothing.
        let index = match track {
            Some(track) => list
                .tracks
                .iter()
                .position(|t| t == track)
                .map(|i| i as i64)
                .unwrap_or(-1),
            None => -1,
        };
        let new_index = index + skip.value();
        if new_index >= 0 && (new_index as usize) < list.tracks.len() {
            list.tracks.get(new_index as usize).cloned()
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// musicplayer/settings/Settings.java -> Settings
//
// All upstream defaults, camelCase gson field names (including the upstream
// "showIngameMenueOverlay" spelling), and the setters' writeToFile-on-change
// behavior: setters mark dirty and the manager flushes (upstream saves inside
// the setter; see the manager note below for why the write is deferred to the
// end of the facade call with identical trigger points).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    volume: i32,
    repeat: Repeat,
    shuffle: bool,
    #[serde(rename = "showIngameOverlay")]
    show_ingame_overlay: bool,
    #[serde(rename = "showIngameMenueOverlay")]
    show_ingame_menue_overlay: bool,
    #[serde(rename = "ingameOverlayPosition")]
    ingame_overlay_position: IngameOverlayPosition,
    #[serde(rename = "keyWorkInGui")]
    key_work_in_gui: bool,
    #[serde(skip)]
    dirty: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            volume: 10,
            repeat: Repeat::No,
            shuffle: false,
            show_ingame_overlay: true,
            show_ingame_menue_overlay: false,
            ingame_overlay_position: IngameOverlayPosition::DownRight,
            key_work_in_gui: true,
            dirty: false,
        }
    }
}

impl Settings {
    pub fn get_volume(&self) -> i32 {
        self.volume
    }

    pub fn set_volume(&mut self, volume: i32) {
        self.volume = volume;
        self.save();
    }

    pub fn get_repeat(&self) -> Repeat {
        self.repeat
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
        self.save();
    }

    pub fn is_shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn set_shuffle(&mut self, shuffle: bool) {
        self.shuffle = shuffle;
        self.save();
    }

    pub fn is_show_ingame_overlay(&self) -> bool {
        self.show_ingame_overlay
    }

    pub fn set_show_ingame_overlay(&mut self, show_ingame_overlay: bool) {
        self.show_ingame_overlay = show_ingame_overlay;
        self.save();
    }

    pub fn is_show_ingame_menue_overlay(&self) -> bool {
        self.show_ingame_menue_overlay
    }

    pub fn set_show_ingame_menue_overlay(&mut self, show_ingame_menue_overlay: bool) {
        self.show_ingame_menue_overlay = show_ingame_menue_overlay;
        self.save();
    }

    pub fn get_ingame_overlay_position(&self) -> IngameOverlayPosition {
        self.ingame_overlay_position
    }

    pub fn set_ingame_overlay_position(&mut self, ingame_overlay_position: IngameOverlayPosition) {
        self.ingame_overlay_position = ingame_overlay_position;
        self.save();
    }

    pub fn is_key_work_in_gui(&self) -> bool {
        self.key_work_in_gui
    }

    pub fn set_key_work_in_gui(&mut self, key_work_in_gui: bool) {
        self.key_work_in_gui = key_work_in_gui;
        self.save();
    }

    pub fn is_finite(&self) -> bool {
        self.repeat == Repeat::No
    }

    pub fn is_single_repeat(&self) -> bool {
        self.repeat == Repeat::Single
    }

    fn save(&mut self) {
        self.dirty = true;
    }
}

// ---------------------------------------------------------------------------
// musicplayer/SettingsManager.java -> SettingsManager (settings.json)
//
// Missing file -> new Settings + write. `gson.fromJson` returning null (an
// empty file) -> new Settings + write. An IOException -> log, settings stays
// absent (upstream leaves it null and later callers would NPE; the port keeps
// the None state instead of crashing). A malformed file is an uncaught
// JsonSyntaxException upstream, so the port panics with the same effect.
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct SettingsManager {
    path: Option<PathBuf>,
    settings: Option<Settings>,
}

impl SettingsManager {
    pub fn new() -> Self {
        SettingsManager::default()
    }

    pub fn set_base_path(&mut self, base_path: &Path) {
        self.path = Some(base_path.join("settings.json"));
    }

    pub fn load_from_file(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if !path.exists() {
            self.settings = Some(Settings::default());
            self.write_to_file();
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                if text.trim().is_empty() {
                    // gson.fromJson on an empty document returns null.
                    self.settings = Some(Settings::default());
                    self.write_to_file();
                    return;
                }
                // Uncaught JsonSyntaxException upstream.
                let settings: Settings = serde_json::from_str(&text).expect("malformed settings.json");
                self.settings = Some(settings);
            }
            Err(ex) => {
                // Upstream logs (with its own "playlist file" typo in the message).
                log_error(&format!("Could not ready playlist file at {}: {}", path.display(), ex));
            }
        }
    }

    /// Writes settings.json. Called by the manager facade after a setter marks
    /// the settings dirty; also called directly for the load paths above.
    pub fn write_to_file(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        if let Some(settings) = self.settings.as_ref() {
            if let Ok(json) = serde_json::to_string_pretty(settings) {
                if let Err(ex) = std::fs::write(path, json) {
                    log_error(&format!("Could not write playlist file at {}: {}", path.display(), ex));
                }
            }
        }
    }

    pub fn take_dirty(&mut self) -> bool {
        let dirty = self.settings.as_ref().map(|s| s.dirty).unwrap_or(false);
        if dirty {
            if let Some(s) = self.settings.as_mut() {
                s.dirty = false;
            }
        }
        dirty
    }

    pub fn get_settings(&self) -> Option<&Settings> {
        self.settings.as_ref()
    }

    pub fn get_settings_mut(&mut self) -> Option<&mut Settings> {
        self.settings.as_mut()
    }
}

// ---------------------------------------------------------------------------
// musicplayer/playlist/Playlists.java -> Playlists
//
// The playlist list plus the playing lock and the currently playing playlist.
// ---------------------------------------------------------------------------

static PLAYLIST_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Default, Clone, Serialize)]
pub struct Playlists {
    playlists: Vec<Playlist>,
    #[serde(skip)]
    locked: bool,
    #[serde(skip)]
    playing: Option<u64>,
    #[serde(skip)]
    dirty: bool,
}

impl<'de> Deserialize<'de> for Playlists {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            playlists: Vec<Playlist>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let mut playlists = Playlists::new();
        for mut playlist in raw.playlists {
            playlist.id = PLAYLIST_COUNTER.fetch_add(1, Ordering::SeqCst);
            playlists.playlists.push(playlist);
        }
        Ok(playlists)
    }
}

impl Playlists {
    pub fn new() -> Self {
        Playlists::default()
    }

    pub fn size(&self) -> usize {
        self.playlists.len()
    }

    pub fn get(&self, index: usize) -> Option<&Playlist> {
        self.playlists.get(index)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut Playlist> {
        self.playlists.get_mut(index)
    }

    pub fn add(&mut self, mut playlist: Playlist) {
        playlist.id = PLAYLIST_COUNTER.fetch_add(1, Ordering::SeqCst);
        self.playlists.push(playlist);
        self.save();
    }

    pub fn remove(&mut self, playlist: &Playlist) {
        self.playlists.retain(|p| p.id != playlist.id);
        self.save();
    }

    pub fn set_playing_lock(&mut self) {
        self.locked = true;
    }

    pub fn remove_playing_lock(&mut self) {
        self.locked = false;
    }

    pub fn is_playing_lock(&self) -> bool {
        self.locked
    }

    pub fn set_playing(&mut self, playing: &Playlist) {
        self.playing = Some(playing.id);
    }

    pub fn get_playing(&self) -> Option<&Playlist> {
        self.playing.and_then(|id| self.playlists.iter().find(|p| p.id == id))
    }

    pub fn get_playing_mut(&mut self) -> Option<&mut Playlist> {
        self.playing.and_then(|id| self.playlists.iter_mut().find(|p| p.id == id))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Playlist> {
        self.playlists.iter()
    }

    pub(crate) fn iter_mut_raw(&mut self) -> impl Iterator<Item = &mut Playlist> {
        self.playlists.iter_mut()
    }

    pub fn take_dirty(&mut self) -> bool {
        let dirty = self.dirty;
        self.dirty = false;
        dirty
    }

    fn save(&mut self) {
        self.dirty = true;
    }
}

// ---------------------------------------------------------------------------
// musicplayer/playlist/Playlist.java -> Playlist
//
// Data half: name + uris (WrappedObject identity semantics), the loaded flag,
// the LoadedTracks list and the mutating methods, each saving like upstream.
//
// Playing half (the ITrackQueue implementation): the next-track state machine.
// `first` means "the track just chosen by set_playable/skip still has to be
// returned once before advancing". Shuffler state is kept between picks like
// upstream's reused java.util.Random; the port keeps a fastrand RNG seeded
// once (seedable for tests) since Java's unseeded Random is not reproducible.
//
// The search backend is injected: upstream resolves uris asynchronously through
// the lavaplayer TrackSearch; the port runs the same placeholder + counter flow
// synchronously over a closure (YouTube/soundcloud resolution is a later slice).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub uris: Vec<WrappedObject>,
    #[serde(skip)]
    id: u64,
    #[serde(skip)]
    loaded: bool,
    #[serde(skip)]
    loaded_tracks: Vec<LoadedTracks>,
    #[serde(skip)]
    dirty: bool,
    // Playing half (never serialized).
    #[serde(skip)]
    next_loaded_track: Option<LoadedTracks>,
    #[serde(skip)]
    next: Option<TrackData>,
    #[serde(skip)]
    first: bool,
    #[serde(skip)]
    shuffle_seed: Option<u64>,
}

impl Default for Playlist {
    fn default() -> Self {
        Playlist {
            name: String::new(),
            uris: Vec::new(),
            id: 0,
            loaded: false,
            loaded_tracks: Vec::new(),
            dirty: false,
            next_loaded_track: None,
            next: None,
            first: false,
            shuffle_seed: None,
        }
    }
}

impl Playlist {
    /// Create a new playlist object with a name.
    pub fn new(name: &str) -> Self {
        Playlist {
            name: name.to_string(),
            ..Default::default()
        }
    }

    /// Only used for gson deserialization upstream; serde builds through
    /// Default too, so deserialized playlists get fresh ids there.
    pub fn set_id(&mut self, id: u64) {
        self.id = id;
    }

    pub fn get_id(&self) -> u64 {
        self.id
    }

    // Data half.

    /// Loads this playlist: dummy entries for every uri, then each uri resolved
    /// through the search backend, replacing its placeholder; the loaded flag
    /// flips when the last entry resolves. Upstream runs this async on a
    /// single-thread executor; the port runs it synchronously and reports the
    /// done callback at the same trigger point.
    pub fn load<F: Fn(&str) -> SearchResultData>(&mut self, search: F, done: &mut dyn FnMut()) {
        if self.loaded {
            return;
        }
        self.unload(); // Unload everything before, as upstream does inside the executor.

        if self.uris.is_empty() {
            self.loaded = true;
            done();
            return;
        }

        let mut counter = 0usize;
        for index in 0..self.uris.len() {
            let uri = self.uris[index].clone();
            self.loaded_tracks.push(LoadedTracks::dummy(uri.clone())); // Add dummy elements
            let result = search(uri.get());
            let loaded_track = LoadedTracks::from_search(uri, result);
            self.loaded_tracks[index] = loaded_track;
            counter += 1;
            if counter == self.uris.len() {
                self.loaded = true;
                done();
            }
        }
    }

    /// Unloads this playlist and removes all loaded tracks.
    pub fn unload(&mut self) {
        self.loaded_tracks.clear();
        self.loaded = false;
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Adds a track to the uri list and the loaded tracks. Must be loaded.
    /// Returns the wrapped uri, or None when not loaded.
    pub fn add_track(&mut self, track: TrackData) -> Option<WrappedObject> {
        if !self.loaded {
            return None;
        }
        let uri = WrappedObject::new(track.uri.clone());
        let index = self.uris.len();
        self.uris.insert(index, uri.clone());
        self.loaded_tracks
            .insert(index, LoadedTracks::from_track(uri.clone(), track));
        self.save();
        Some(uri)
    }

    /// Adds a track list to the uri list and the loaded tracks if it has a
    /// valid uri and is not a search result. Must be loaded.
    pub fn add_track_list(&mut self, track_list: TrackListData) -> Option<WrappedObject> {
        if !self.loaded {
            return None;
        }
        if !track_list.is_search() && track_list.has_uri() {
            let uri = WrappedObject::new(track_list.uri.clone().unwrap());
            let index = self.uris.len();
            self.uris.insert(index, uri.clone());
            self.loaded_tracks
                .insert(index, LoadedTracks::from_track_list(uri.clone(), track_list));
            self.save();
            return Some(uri);
        }
        None
    }

    /// Removes a uri from the uri list and the loaded tracks. Must be loaded.
    pub fn remove(&mut self, uri: &WrappedObject) -> bool {
        if !self.loaded {
            return false;
        }
        if let Some(index) = self.uris.iter().position(|u| u == uri) {
            self.uris.remove(index);
            self.loaded_tracks.remove(index);
            self.save();
            true
        } else {
            false
        }
    }

    /// Move a uri and its loaded track up (positive value) or down. Must be
    /// loaded. Upstream: newIndex = oldIndex - value.
    pub fn move_uri(&mut self, uri: &WrappedObject, value: i64) -> bool {
        if !self.loaded {
            return false;
        }
        let Some(old_index) = self.uris.iter().position(|u| u == uri) else {
            return false;
        };
        let new_index = old_index as i64 - value;
        if new_index >= 0 && (new_index as usize) < self.uris.len() {
            let uri = self.uris.remove(old_index);
            self.uris.insert(new_index as usize, uri);
            let loaded = self.loaded_tracks.remove(old_index);
            self.loaded_tracks.insert(new_index as usize, loaded);
            self.save();
            true
        } else {
            false
        }
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
        self.save();
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn get_entry_size(&self) -> usize {
        self.uris.len()
    }

    pub fn get_loaded_tracks(&self) -> &[LoadedTracks] {
        &self.loaded_tracks
    }

    /// True if the playlist does not contain any uris.
    pub fn is_empty(&self) -> bool {
        self.uris.is_empty()
    }

    pub fn take_dirty(&mut self) -> bool {
        let dirty = self.dirty;
        self.dirty = false;
        dirty
    }

    fn save(&mut self) {
        self.dirty = true;
    }

    // Playing half. Nothing here is serializable.

    /// ITrackQueue.calculateNext: reads the current settings from the manager
    /// like upstream.
    pub fn calculate_next(&mut self) -> bool {
        let settings = manager::settings_snapshot();
        self.calculate_next_with(&settings)
    }

    pub fn calculate_next_with(&mut self, settings: &Settings) -> bool {
        if self.next_loaded_track.is_none() || self.next.is_none() {
            false
        } else if self.first {
            self.first = false;
            true
        } else if settings.is_single_repeat() {
            true
        } else if settings.is_shuffle() {
            self.select_random_track()
        } else {
            self.find_next_song(settings, Skip::Forward)
        }
    }

    pub fn get_next(&self) -> Option<TrackData> {
        self.next.clone()
    }

    /// Skip the current song in the skip direction. Upstream assigns the
    /// result to the `first` field as well.
    pub fn skip(&mut self, skip: Skip, settings: &Settings) -> bool {
        self.first = if settings.is_shuffle() {
            self.select_random_track()
        } else {
            self.find_next_song(settings, skip)
        };
        self.first
    }

    /// The first track pair in this playlist; errored and unresolved entries
    /// are skipped, values may be None when empty or all invalid.
    pub fn get_first_track(&self) -> (Option<LoadedTracks>, Option<TrackData>) {
        for loaded_track in &self.loaded_tracks {
            if let Some(track) = loaded_track.get_first_track() {
                return (Some(loaded_track.clone()), Some(track));
            }
        }
        (None, None)
    }

    /// The last track pair in this playlist; errored and unresolved entries
    /// are skipped, values may be None when empty or all invalid.
    pub fn get_last_track(&self) -> (Option<LoadedTracks>, Option<TrackData>) {
        for loaded_track in self.loaded_tracks.iter().rev() {
            if let Some(track) = loaded_track.get_last_track() {
                return (Some(loaded_track.clone()), Some(track));
            }
        }
        (None, None)
    }

    /// Sets the start loaded track with the contained track.
    pub fn set_playable(&mut self, loaded_track: LoadedTracks, track: TrackData) {
        self.set_tracks(loaded_track, track);
        self.first = true;
    }

    /// Sets the next track to None, so the queue, if playing, stops.
    pub fn set_stopable(&mut self) {
        self.next_loaded_track = None;
        self.next = None;
    }

    /// The pair one step after (or before, for Skip::Previous) the current
    /// track. Unresolved or errored entries are skipped over; reaching the end
    /// (or start) yields (None, None).
    fn get_other_track(&self, loaded_track: Option<&LoadedTracks>, track: Option<&TrackData>, skip: Skip) -> (Option<LoadedTracks>, Option<TrackData>) {
        let Some(loaded_track) = loaded_track else {
            return (None, None);
        };
        if let Some(next_track) = loaded_track.get_other_track(track, skip) {
            return (Some(loaded_track.clone()), Some(next_track));
        }
        let Some(index) = self.loaded_tracks.iter().position(|lt| lt == loaded_track) else {
            return (None, None);
        };

        let index = index as i64;
        let mut new_index = index + skip.value();
        loop {
            let in_bounds = if skip == Skip::Forward {
                new_index < self.loaded_tracks.len() as i64
            } else {
                new_index >= 0
            };
            if !in_bounds {
                break;
            }
            if let Some(next_valid) = self.get_track_and_validate(new_index as usize) {
                let track = if skip == Skip::Forward {
                    next_valid.get_first_track()
                } else {
                    next_valid.get_last_track()
                };
                return (Some(next_valid), track);
            }
            new_index += skip.value();
        }
        (None, None)
    }

    /// Get a LoadedTracks at the index and test that it has no error and holds
    /// a valid track or track list. Returns None if the test failed.
    ///
    /// Upstream guard is `index <= 0 && index >= loadedTracks.size()` (should
    /// be `||`); it is kept 1:1, unreachable in practice because callers pass
    /// in-bounds indexes derived from a found entry.
    fn get_track_and_validate(&self, index: usize) -> Option<LoadedTracks> {
        if index <= 0 && index >= self.loaded_tracks.len() {
            return None;
        }
        let loaded_track = self.loaded_tracks.get(index)?;
        if loaded_track.has_error() || (!loaded_track.is_track() && !loaded_track.is_track_list()) {
            return None;
        }
        Some(loaded_track.clone())
    }

    fn set_tracks(&mut self, loaded_track: LoadedTracks, track: TrackData) {
        self.next_loaded_track = Some(loaded_track);
        self.next = Some(track);
    }

    /// Find a next song in the skip direction and set it as the next track.
    /// Returns true when a valid next song was found.
    fn find_next_song(&mut self, settings: &Settings, skip: Skip) -> bool {
        let pair = self.get_other_track(self.next_loaded_track.as_ref(), self.next.as_ref(), skip);
        let loaded_track = pair.0;
        let track = pair.1;

        if loaded_track.is_none() || track.is_none() {
            if settings.is_finite() {
                false
            } else if !self.loaded_tracks.is_empty() {
                let side_pair = if skip == Skip::Forward {
                    self.get_first_track()
                } else {
                    self.get_last_track()
                };
                let side_loaded_track = side_pair.0;
                let side_track = side_pair.1;
                if side_loaded_track.is_some() && side_track.is_some() {
                    self.set_tracks(side_loaded_track.unwrap(), side_track.unwrap());
                    true
                } else if let Some(side_loaded_track) = side_loaded_track {
                    let next_valid_pair = self.get_other_track(Some(&side_loaded_track), None, skip);
                    if next_valid_pair.0.is_some() && next_valid_pair.1.is_some() {
                        self.set_tracks(next_valid_pair.0.unwrap(), next_valid_pair.1.unwrap());
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            self.set_tracks(loaded_track.unwrap(), track.unwrap());
            true
        }
    }

    /// Select a random track (shuffle mode): every plain track and every track
    /// inside a track list is a candidate.
    fn select_random_track(&mut self) -> bool {
        let mut shuffle_entries: Vec<(LoadedTracks, TrackData)> = Vec::new();
        for loaded_track in &self.loaded_tracks {
            if loaded_track.is_track() {
                if let Some(track) = loaded_track.get_track() {
                    shuffle_entries.push((loaded_track.clone(), track.clone()));
                }
            } else if loaded_track.is_track_list() {
                if let Some(list) = loaded_track.get_track_list() {
                    for track in &list.tracks {
                        shuffle_entries.push((loaded_track.clone(), track.clone()));
                    }
                }
            }
        }
        if shuffle_entries.is_empty() {
            return false;
        }
        let seed = self.shuffle_seed.unwrap_or_else(|| fastrand::u64(..));
        let mut rng = fastrand::Rng::with_seed(seed);
        // Fisher-Yates like Collections.shuffle.
        for i in (1..shuffle_entries.len()).rev() {
            let j = rng.usize(..=i);
            shuffle_entries.swap(i, j);
        }
        let pick = rng.usize(..shuffle_entries.len());
        // The reused Random upstream keeps drawing from a live stream; chain
        // the next seed from it so repeated picks stay deterministic.
        let next_seed = rng.u64(..);
        self.shuffle_seed = Some(next_seed);
        let (loaded_track, track) = shuffle_entries.remove(pick);
        self.next_loaded_track = Some(loaded_track);
        self.next = Some(track);
        true
    }
}

// ---------------------------------------------------------------------------
// musicplayer/PlaylistManager.java -> PlaylistManager (playlist.json.gz)
//
// Gzip-wrapped JSON, like upstream. Missing file -> new Playlists + write.
// A gzip corruption (ZipException) -> warn + new Playlists. An IOException ->
// log, playlists stays absent. A malformed (un-gzipped) payload is upstream's
// ZipException path, so it lands on the corrupted-file branch too.
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct PlaylistManager {
    path: Option<PathBuf>,
    playlists: Option<Playlists>,
}

impl PlaylistManager {
    pub fn new() -> Self {
        PlaylistManager::default()
    }

    pub fn set_base_path(&mut self, base_path: &Path) {
        self.path = Some(base_path.join("playlist.json.gz"));
    }

    pub fn load_from_file(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if !path.exists() {
            self.playlists = Some(Playlists::new());
            self.write_to_file();
            return;
        }
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(ex) => {
                log_error(&format!("Could not read playlist file at {}: {}", path.display(), ex));
                return;
            }
        };
        let mut reader = flate2::read::GzDecoder::new(file);
        if reader.header().is_none() {
            // Not gzip: the upstream ZipException branch.
            debug_log(&format!("The playlist file is corrupted and will be deleted at {}", path.display()));
            self.playlists = Some(Playlists::new());
            return;
        }
        let mut text = String::new();
        match std::io::Read::read_to_string(&mut reader, &mut text) {
            Ok(_) => {
                // Malformed JSON inside valid gzip: also the ZipException branch,
                // since upstream's gzip stream would fail to decode cleanly.
                match serde_json::from_str(&text) {
                    Ok(playlists) => self.playlists = Some(playlists),
                    Err(_) => {
                        debug_log(&format!("The playlist file is corrupted and will be deleted at {}", path.display()));
                        self.playlists = Some(Playlists::new());
                    }
                }
            }
            Err(ex) => {
                log_error(&format!("Could not read playlist file at {}: {}", path.display(), ex));
            }
        }
    }

    /// Writes playlist.json.gz. Called by the manager facade after a data
    /// mutation marks the playlists dirty, and directly by the load paths.
    pub fn write_to_file(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let Some(playlists) = self.playlists.as_ref() else {
            return;
        };
        let Ok(json) = serde_json::to_string_pretty(playlists) else {
            return;
        };
        let file = match std::fs::File::create(path) {
            Ok(file) => file,
            Err(ex) => {
                log_error(&format!("Could not write playlist file at {}: {}", path.display(), ex));
                return;
            }
        };
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        if let Err(ex) = encoder.write_all(json.as_bytes()) {
            log_error(&format!("Could not write playlist file at {}: {}", path.display(), ex));
            return;
        }
        if let Err(ex) = encoder.finish() {
            log_error(&format!("Could not write playlist file at {}: {}", path.display(), ex));
        }
    }

    pub fn get_playlists(&self) -> Option<&Playlists> {
        self.playlists.as_ref()
    }

    pub fn get_playlists_mut(&mut self) -> Option<&mut Playlists> {
        self.playlists.as_mut()
    }
}

// ---------------------------------------------------------------------------
// lavaplayer queue (slice 1).
//
// TrackQueue is the ITrackQueue surface (calculateNext/getNext). The
// AudioPlayer stand-in keeps the playing track, its paused flag and the
// volume: the decode/stream half is a later slice.
// ---------------------------------------------------------------------------

pub trait TrackQueue {
    fn calculate_next(&mut self) -> bool;
    fn get_next(&self) -> Option<TrackData>;
}

impl TrackQueue for Playlist {
    fn calculate_next(&mut self) -> bool {
        Playlist::calculate_next(self)
    }

    fn get_next(&self) -> Option<TrackData> {
        Playlist::get_next(self)
    }
}

#[derive(Debug, Default, Clone)]
pub struct AudioPlayerState {
    playing: Option<TrackData>,
    paused: bool,
    volume: i32,
}

impl AudioPlayerState {
    /// lavaplayer AudioPlayer.startTrack(track, noInterrupt): with a track
    /// already playing and noInterrupt set, the start is refused (returns
    /// false) instead of interrupting.
    pub fn start_track(&mut self, track: TrackData, no_interrupt: bool) -> bool {
        if self.playing.is_some() && no_interrupt {
            return false;
        }
        self.playing = Some(track);
        true
    }

    pub fn stop_track(&mut self) {
        self.playing = None;
    }

    pub fn get_playing_track(&self) -> Option<&TrackData> {
        self.playing.as_ref()
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_volume(&mut self, volume: i32) {
        self.volume = volume;
    }

    pub fn get_volume(&self) -> i32 {
        self.volume
    }
}

/// AudioTrackEndReason, narrowed to what TrackManager reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackEndReason {
    Finished,
    LoadFailed,
    Other,
}

/// queue/TrackManager.java -> TrackManager over the stand-in player.
#[derive(Default)]
pub struct TrackManager {
    audio: AudioPlayerState,
    queue_wrapper: Option<Box<dyn TrackQueue + Send>>,
}

impl std::fmt::Debug for TrackManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackManager")
            .field("paused", &self.audio.is_paused())
            .field("has_queue", &self.queue_wrapper.is_some())
            .finish()
    }
}

impl TrackManager {
    pub fn new() -> Self {
        TrackManager::default()
    }

    /// AudioEventAdapter.onTrackEnd: FINISHED or LOAD_FAILED advance the queue.
    pub fn on_track_end(&mut self, end_reason: TrackEndReason) {
        if end_reason == TrackEndReason::Finished || end_reason == TrackEndReason::LoadFailed {
            self.skip();
        }
    }

    pub fn start(&mut self) {
        self.set_paused(false);
        self.skip();
    }

    pub fn stop(&mut self) {
        self.set_paused(false);
        self.audio.stop_track();
        self.queue_wrapper = None;
    }

    pub fn set_track_queue(&mut self, queue: Option<Box<dyn TrackQueue + Send>>) {
        self.queue_wrapper = queue;
    }

    pub fn skip(&mut self) {
        let Some(queue) = self.queue_wrapper.as_mut() else {
            self.stop();
            return;
        };
        if queue.calculate_next() && queue.get_next().is_some() {
            self.audio.start_track(queue.get_next().unwrap(), false);
        } else {
            self.stop();
        }
    }

    pub fn set_paused(&mut self, value: bool) {
        self.audio.set_paused(value);
    }

    pub fn is_paused(&self) -> bool {
        self.audio.is_paused()
    }

    pub fn get_current_track(&self, current_track_position: i64) -> Option<TrackData> {
        self.audio
            .get_playing_track()
            .map(|track| PlayingTrackView::view(track, current_track_position))
    }

    /// PlayingTrackImpl.setPosition applied to the playing track.
    pub fn set_current_position(&mut self, position: i64, current_track_position: &mut i64) {
        if let Some(track) = self.audio.playing.as_mut() {
            track.set_position(position, current_track_position);
        }
    }

    pub fn audio(&self) -> &AudioPlayerState {
        &self.audio
    }

    pub fn audio_mut(&mut self) -> &mut AudioPlayerState {
        &mut self.audio
    }
}

/// The IPlayingTrack view of the playing track.
#[derive(Debug, Clone)]
pub struct PlayingTrackView {
    track: TrackData,
}

impl PlayingTrackView {
    fn view(track: &TrackData, current_track_position: i64) -> TrackData {
        let mut view = track.clone();
        view.position_ms = track.position_at(current_track_position);
        view
    }

    pub fn get_uri(&self) -> &str {
        &self.track.uri
    }

    pub fn get_title(&self) -> &str {
        &self.track.title
    }

    pub fn get_duration(&self) -> i64 {
        self.track.duration_ms
    }

    pub fn get_position(&self) -> i64 {
        self.track.position_ms
    }

    pub fn is_stream(&self) -> bool {
        self.track.is_stream
    }

    pub fn into_track(self) -> TrackData {
        self.track
    }
}

/// lavaplayer/MusicPlayer.java -> MusicPlayerState (data slice).
///
/// The decode/stream engine, audio output device handling and the timescale
/// speed/pitch filters are later slices; the state (volume, speed, pitch,
/// currentTrackPosition tracking) is kept with the same clamps.
#[derive(Debug, Default)]
pub struct MusicPlayerState {
    current_track_position: i64,
    speed: f32,
    pitch: f32,
    volume: i32,
}

impl MusicPlayerState {
    pub fn new() -> Self {
        MusicPlayerState {
            current_track_position: 0,
            speed: 1.0,
            pitch: 1.0,
            volume: 0,
        }
    }

    pub fn get_current_track_position(&self) -> i64 {
        self.current_track_position
    }

    pub fn set_current_track_position(&mut self, current_track_position: i64) {
        self.current_track_position = current_track_position;
    }

    /// setSpeed: clamped to [0.05, 10] like upstream.
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.05, 10.0);
    }

    pub fn get_speed(&self) -> f32 {
        self.speed
    }

    /// setPitch: clamped to [0.05, 10] like upstream.
    pub fn set_pitch(&mut self, pitch: f32) {
        self.pitch = pitch.clamp(0.05, 10.0);
    }

    pub fn get_pitch(&self) -> f32 {
        self.pitch
    }

    pub fn set_volume(&mut self, volume: i32) {
        // Upstream applies the volume straight to the AudioPlayer. The stand-in
        // player lives inside the TrackManager, so the real application is the
        // render half's job; storing it here keeps the lock order one-way
        // (track_manager -> player) and cannot deadlock against manager::setup.
        self.volume = volume;
    }

    pub fn get_volume(&self) -> i32 {
        self.volume
    }
}

// ---------------------------------------------------------------------------
// musicplayer/MusicPlayerUtils.java -> skip_forward / skip_back
// ---------------------------------------------------------------------------

pub fn skip_forward() {
    let tm = manager::track_manager_lock();
    let mut track_manager = tm.lock().unwrap();
    if manager::skip_playing(Skip::Forward) {
        track_manager.skip();
    }
}

pub fn skip_back() {
    let tm = manager::track_manager_lock();
    let mut track_manager = tm.lock().unwrap();
    let player = manager::player_lock();
    let mut player = player.lock().unwrap();
    let Some(currently_playing) = track_manager.get_current_track(player.get_current_track_position()) else {
        return;
    };

    let mut max_duration = currently_playing.get_duration() / 10;
    if max_duration > 10000 {
        max_duration = 10000;
    }

    if currently_playing.get_position() > max_duration && !currently_playing.is_stream() {
        track_manager.set_current_position(0, &mut player.current_track_position);
    } else if manager::skip_playing(Skip::Previous) {
        track_manager.skip();
    } else {
    }
}

// ---------------------------------------------------------------------------
// musicplayer/MusicPlayerManager.java -> manager (global singletons)
//
// Upstream holds static singletons; the port keeps the same singletons behind
// mutexes. setup() mirrors MusicPlayerManager.setup: load the file layout,
// point both managers at the directory, load playlist.json.gz and
// settings.json, then apply the settings volume to the player. Facade helpers
// (with_settings / with_playlists) run a mutation and flush the file right
// after, at the same trigger points as upstream's in-setter save.
// ---------------------------------------------------------------------------

pub mod manager {
    use super::*;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    static FILES: OnceLock<Mutex<MusicPlayerFiles>> = OnceLock::new();
    static PLAYLIST_MANAGER: OnceLock<Mutex<PlaylistManager>> = OnceLock::new();
    static SETTINGS_MANAGER: OnceLock<Mutex<SettingsManager>> = OnceLock::new();
    static PLAYER: OnceLock<Mutex<MusicPlayerState>> = OnceLock::new();
    static TRACK_MANAGER: OnceLock<Mutex<TrackManager>> = OnceLock::new();

    pub fn files_lock() -> &'static Mutex<MusicPlayerFiles> {
        FILES.get_or_init(|| Mutex::new(MusicPlayerFiles::new()))
    }

    pub fn playlist_manager_lock() -> &'static Mutex<PlaylistManager> {
        PLAYLIST_MANAGER.get_or_init(|| Mutex::new(PlaylistManager::new()))
    }

    pub fn settings_manager_lock() -> &'static Mutex<SettingsManager> {
        SETTINGS_MANAGER.get_or_init(|| Mutex::new(SettingsManager::new()))
    }

    pub fn player_lock() -> &'static Mutex<MusicPlayerState> {
        PLAYER.get_or_init(|| Mutex::new(MusicPlayerState::new()))
    }

    pub fn track_manager_lock() -> &'static Mutex<TrackManager> {
        TRACK_MANAGER.get_or_init(|| Mutex::new(TrackManager::new()))
    }

    /// MusicPlayerManager.setup: fixLogLevel and generatePlayer are engine
    /// work (later slices); the file layout, both managers and the settings
    /// volume apply here.
    pub fn setup(internal_playlists: bool, game_dir: &Path) {
        {
            let files = files_lock();
            let mut files = files.lock().unwrap();
            files.load(internal_playlists, game_dir);
        }
        let directory = files_lock()
            .lock()
            .unwrap()
            .get_directory()
            .map(|p| p.to_path_buf())
            .expect("music player directory not set");

        {
            let pm = playlist_manager_lock();
            let mut pm = pm.lock().unwrap();
            pm.set_base_path(&directory);
            pm.load_from_file();
        }
        let volume = {
            let sm = settings_manager_lock();
            let mut sm = sm.lock().unwrap();
            sm.set_base_path(&directory);
            sm.load_from_file();
            sm.get_settings().map(|s| s.get_volume()).unwrap_or(0)
        };
        player_lock().lock().unwrap().set_volume(volume);
    }

    pub fn settings_snapshot() -> Settings {
        let sm = settings_manager_lock();
        let sm = sm.lock().unwrap();
        sm.get_settings().cloned().unwrap_or_default()
    }

    /// Runs a settings mutation, then flushes settings.json when a setter
    /// marked it dirty (upstream saves inside the setter).
    pub fn with_settings<R>(f: impl FnOnce(&mut Settings) -> R) -> Option<R> {
        let sm = settings_manager_lock();
        let mut sm = sm.lock().unwrap();
        let result = f(sm.get_settings_mut()?);
        if sm.take_dirty() {
            sm.write_to_file();
        }
        Some(result)
    }

    /// The queue handle the track manager plays: the currently playing
    /// playlist itself, like upstream (TrackManager wraps the same Playlist
    /// object that the PlaylistManager owns; the port reaches it through the
    /// manager so there are never two copies).
    pub struct PlayingPlaylistQueue;

    impl TrackQueue for PlayingPlaylistQueue {
        fn calculate_next(&mut self) -> bool {
            let pm = playlist_manager_lock();
            let mut pm = pm.lock().unwrap();
            let Some(playlists) = pm.get_playlists_mut() else {
                return false;
            };
            let Some(playlist) = playlists.get_playing_mut() else {
                return false;
            };
            playlist.calculate_next()
        }

        fn get_next(&self) -> Option<TrackData> {
            let pm = playlist_manager_lock();
            let pm = pm.lock().unwrap();
            let playlists = pm.get_playlists()?;
            let playlist = playlists.get_playing()?;
            playlist.get_next()
        }
    }

    pub fn playing_queue() -> Box<dyn TrackQueue + Send> {
        Box::new(PlayingPlaylistQueue)
    }

    /// Playlist.skip on the currently playing playlist (the TrackManager's
    /// queue), reading the shared settings. Lock order stays
    /// track_manager -> playlist_manager -> settings_manager everywhere.
    pub fn skip_playing(skip: Skip) -> bool {
        let pm = playlist_manager_lock();
        let mut pm = pm.lock().unwrap();
        let sm = settings_manager_lock();
        let sm = sm.lock().unwrap();
        let Some(settings) = sm.get_settings() else {
            return false;
        };
        let Some(playlists) = pm.get_playlists_mut() else {
            return false;
        };
        let Some(playlist) = playlists.get_playing_mut() else {
            return false;
        };
        playlist.skip(skip, settings)
    }

    /// Runs a playlists mutation, then flushes playlist.json.gz when anything
    /// (the list itself or a playlist) marked itself dirty.
    pub fn with_playlists<R>(f: impl FnOnce(&mut Playlists) -> R) -> Option<R> {
        let pm = playlist_manager_lock();
        let mut pm = pm.lock().unwrap();
        let result = f(pm.get_playlists_mut()?);
        let mut dirty = false;
        if let Some(playlists) = pm.get_playlists_mut() {
            dirty |= playlists.take_dirty();
            for playlist in playlists.iter_mut_raw() {
                dirty |= playlist.take_dirty();
            }
        }
        if dirty {
            pm.write_to_file();
        }
        Some(result)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    // The singletons are process-global (as upstream's are), so tests that
    // mutate shared settings serialize here.
    static TEST_SERIALIZER: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_SERIALIZER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    use super::*;

    fn track(uri: &str, title: &str, duration_ms: i64) -> TrackData {
        TrackData::new(uri, title, duration_ms, false)
    }

    fn list(name: &str, uri: &str, tracks: Vec<TrackData>) -> TrackListData {
        TrackListData {
            uri: if uri.is_empty() { None } else { Some(uri.to_string()) },
            ..TrackListData::named(name, tracks)
        }
    }

    fn dummy_playlist() -> Playlist {
        Playlist::new("test")
    }

    // WrappedObject.

    #[test]
    fn wrapped_objects_with_the_same_value_are_not_equal() {
        let _guard = test_guard();
        let a = WrappedObject::new("same".to_string());
        let b = WrappedObject::new("same".to_string());
        assert_ne!(a, b);
        assert_eq!(a, a.clone());
        assert_eq!(a.get(), "same");
    }

    #[test]
    fn wrapped_object_serializes_as_the_gson_o_field() {
        let _guard = test_guard();
        let wrapper = WrappedObject::new("uri".to_string());
        let json = serde_json::to_string(&wrapper).unwrap();
        assert_eq!(json, r#"{"o":"uri"}"#);
        let back: WrappedObject = serde_json::from_str(&json).unwrap();
        assert_eq!(back.get(), "uri");
        assert_ne!(back, wrapper); // Deserialization is a fresh identity, like gson.
        let alt: WrappedObject = serde_json::from_str(r#"{"object":"uri"}"#).unwrap();
        assert_eq!(alt.get(), "uri");
    }

    // ClientConfig.

    #[test]
    fn client_config_defaults_to_external_playlists_and_round_trips() {
        let _guard = test_guard();
        let config = ClientConfig::default();
        assert!(!config.internal_playlists);
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(json, r#"{"internalPlaylists":false}"#);
        let back: ClientConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.internal_playlists, false);
    }

    #[test]
    fn client_config_missing_file_is_written_with_the_default() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!("mp-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("musicplayer.json");
        let config = ClientConfig::load(&path);
        assert!(!config.internal_playlists);
        assert!(path.exists());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("internalPlaylists"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn client_config_loads_the_saved_value() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!("mp-cfg2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("musicplayer.json");
        std::fs::write(&path, r#"{"internalPlaylists":true}"#).unwrap();
        let config = ClientConfig::load(&path);
        assert!(config.internal_playlists);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Enums and utilities.

    #[test]
    fn repeat_cycles_and_is_active() {
        let _guard = test_guard();
        assert!(!Repeat::No.is_active());
        assert!(Repeat::Playlist.is_active());
        assert!(Repeat::Single.is_active());
        assert_eq!(Repeat::No.forward_cycle(), Repeat::Playlist);
        assert_eq!(Repeat::Playlist.forward_cycle(), Repeat::Single);
        assert_eq!(Repeat::Single.forward_cycle(), Repeat::No);
    }

    #[test]
    fn ingame_overlay_position_cycles_and_sides() {
        let _guard = test_guard();
        assert!(IngameOverlayPosition::UpLeft.is_up() && IngameOverlayPosition::UpLeft.is_left());
        assert!(IngameOverlayPosition::UpRight.is_up() && !IngameOverlayPosition::UpRight.is_left());
        assert!(!IngameOverlayPosition::DownRight.is_up() && !IngameOverlayPosition::DownRight.is_left());
        assert!(!IngameOverlayPosition::DownLeft.is_up() && IngameOverlayPosition::DownLeft.is_left());
        assert_eq!(
            IngameOverlayPosition::UpLeft.forward_cycle(),
            IngameOverlayPosition::UpRight
        );
        assert_eq!(
            IngameOverlayPosition::DownLeft.forward_cycle(),
            IngameOverlayPosition::UpLeft
        );
        assert_eq!(
            IngameOverlayPosition::DownRight.localization(),
            "gui.settings.position.down_right"
        );
    }

    #[test]
    fn skip_values_match_upstream() {
        let _guard = test_guard();
        assert_eq!(Skip::Forward.value(), 1);
        assert_eq!(Skip::Previous.value(), -1);
    }

    #[test]
    fn time_conversion_matches_the_upstream_format() {
        let _guard = test_guard();
        assert_eq!(time_conversion(0), "00:00");
        assert_eq!(time_conversion(59), "00:59");
        assert_eq!(time_conversion(60), "01:00");
        assert_eq!(time_conversion(61), "01:01");
        assert_eq!(time_conversion(3600), "1:00:00");
        assert_eq!(time_conversion(3661), "1:01:01");
        assert_eq!(time_conversion(7325), "2:02:05");
    }

    // Settings.

    #[test]
    fn settings_defaults_match_upstream() {
        let _guard = test_guard();
        let settings = Settings::default();
        assert_eq!(settings.get_volume(), 10);
        assert_eq!(settings.get_repeat(), Repeat::No);
        assert!(!settings.is_shuffle());
        assert!(settings.is_show_ingame_overlay());
        assert!(!settings.is_show_ingame_menue_overlay());
        assert_eq!(settings.get_ingame_overlay_position(), IngameOverlayPosition::DownRight);
        assert!(settings.is_key_work_in_gui());
        assert!(settings.is_finite());
        assert!(!settings.is_single_repeat());
    }

    #[test]
    fn settings_serializes_with_the_upstream_camel_case_names() {
        let _guard = test_guard();
        let json = serde_json::to_string(&Settings::default()).unwrap();
        for name in [
            "volume",
            "repeat",
            "shuffle",
            "showIngameOverlay",
            "showIngameMenueOverlay",
            "ingameOverlayPosition",
            "keyWorkInGui",
        ] {
            assert!(json.contains(&format!(r#""{}""#, name)), "missing field {}", name);
        }
    }

    #[test]
    fn settings_round_trips_through_json() {
        let _guard = test_guard();
        let mut settings = Settings::default();
        settings.set_repeat(Repeat::Single);
        settings.set_shuffle(true);
        settings.set_ingame_overlay_position(IngameOverlayPosition::UpLeft);
        let json = serde_json::to_string(&settings).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.get_repeat(), Repeat::Single);
        assert!(back.is_shuffle());
        assert_eq!(back.get_ingame_overlay_position(), IngameOverlayPosition::UpLeft);
    }

    #[test]
    fn settings_manager_missing_file_creates_and_writes_defaults() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!("mp-sm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut manager = SettingsManager::new();
        manager.set_base_path(&dir);
        manager.load_from_file();
        let settings = manager.get_settings().unwrap();
        assert_eq!(settings.get_volume(), 10);
        assert!(dir.join("settings.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_manager_empty_file_falls_back_to_defaults_and_writes() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!("mp-sm2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "").unwrap();
        let mut manager = SettingsManager::new();
        manager.set_base_path(&dir);
        manager.load_from_file();
        assert_eq!(manager.get_settings().unwrap().get_volume(), 10);
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(!text.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // LoadedTracks.

    #[test]
    fn loaded_tracks_from_track_and_list_and_error() {
        let _guard = test_guard();
        let uri = WrappedObject::new("u1".to_string());

        let from_track = LoadedTracks::from_track(uri.clone(), track("u1", "Song", 100_000));
        assert!(from_track.is_track());
        assert!(!from_track.is_track_list());
        assert_eq!(from_track.get_title(), Some("Song"));
        assert_eq!(from_track.get_first_track().unwrap().title, "Song");
        assert_eq!(from_track.get_last_track().unwrap().title, "Song");
        assert!(from_track
            .get_other_track(Some(&track("u1", "Song", 100_000)), Skip::Forward)
            .is_none());

        let from_list = LoadedTracks::from_track_list(
            uri.clone(),
            list("Album", "u2", vec![track("a", "One", 1), track("b", "Two", 2)]),
        );
        assert!(from_list.is_track_list());
        assert_eq!(from_list.get_title(), Some("Album"));
        assert_eq!(from_list.get_first_track().unwrap().title, "One");
        assert_eq!(from_list.get_last_track().unwrap().title, "Two");

        let from_error = LoadedTracks::from_search(uri.clone(), SearchResultData::errored("Nope"));
        assert!(from_error.has_error());
        assert_eq!(from_error.get_error_message(), Some("Nope"));
        assert!(from_error.get_first_track().is_none());

        let dummy = LoadedTracks::dummy(uri.clone());
        assert!(dummy.has_error());
        assert_eq!(dummy.get_error_message(), Some("Not loaded yet"));
    }

    #[test]
    fn loaded_tracks_get_other_track_steps_by_skip() {
        let _guard = test_guard();
        let uri = WrappedObject::new("u".to_string());
        let tracks = vec![track("a", "One", 1), track("b", "Two", 2), track("c", "Three", 3)];
        let loaded = LoadedTracks::from_track_list(uri, list("Album", "u", tracks));

        let two = track("b", "Two", 2);
        let forward = loaded.get_other_track(Some(&two), Skip::Forward).unwrap();
        assert_eq!(forward.title, "Three");
        let back = loaded.get_other_track(Some(&two), Skip::Previous).unwrap();
        assert_eq!(back.title, "One");

        // End of the list: no next track.
        let last = track("c", "Three", 3);
        assert!(loaded.get_other_track(Some(&last), Skip::Forward).is_none());
        let first = track("a", "One", 1);
        assert!(loaded.get_other_track(Some(&first), Skip::Previous).is_none());

        // A missing track behaves like Java's indexOf(null) = -1: FORWARD reads
        // index 0, PREVIOUS reads -2 and finds nothing.
        let from_null_forward = loaded.get_other_track(None, Skip::Forward).unwrap();
        assert_eq!(from_null_forward.title, "One");
        assert!(loaded.get_other_track(None, Skip::Previous).is_none());
    }

    // Playlist data half.

    #[test]
    fn playlist_mutations_require_load() {
        let _guard = test_guard();
        let mut playlist = dummy_playlist();
        assert!(!playlist.is_loaded());
        assert!(playlist.add_track(track("a", "One", 1)).is_none());
        assert!(playlist.add_track_list(list("L", "u", vec![])).is_none());
        assert!(!playlist.remove(&WrappedObject::new("a".to_string())));
        assert_eq!(playlist.get_entry_size(), 0);
        assert!(playlist.is_empty());
    }

    #[test]
    fn playlist_load_resolves_dummies_through_the_search_backend() {
        let _guard = test_guard();
        let mut playlist = dummy_playlist();
        playlist.uris = vec![
            WrappedObject::new("a".to_string()),
            WrappedObject::new("b".to_string()),
            WrappedObject::new("c".to_string()),
        ];
        let mut done = 0;
        playlist.load(
            |uri| {
                if uri == "c" {
                    SearchResultData::errored("failed")
                } else {
                    SearchResultData::of_track(track(uri, uri, 10_000))
                }
            },
            &mut || done += 1,
        );
        assert!(playlist.is_loaded());
        assert_eq!(done, 1);
        assert_eq!(playlist.get_entry_size(), 3);
        let loaded = playlist.get_loaded_tracks();
        assert!(loaded[0].is_track());
        assert_eq!(loaded[0].get_title(), Some("a"));
        assert!(loaded[2].has_error());
        // Loading twice is a no-op, like upstream.
        let before = playlist.get_loaded_tracks().len();
        playlist.load(|_| SearchResultData::errored("no"), &mut || done += 1);
        assert_eq!(playlist.get_loaded_tracks().len(), before);
        assert_eq!(done, 1);
    }

    #[test]
    fn playlist_load_empty_playlist_sets_loaded_and_calls_done() {
        let _guard = test_guard();
        let mut playlist = dummy_playlist();
        let mut done = 0;
        playlist.load(|_| SearchResultData::errored("no"), &mut || done += 1);
        assert!(playlist.is_loaded());
        assert_eq!(done, 1);
    }

    #[test]
    fn playlist_add_track_and_track_list_assign_wrapped_uris() {
        let _guard = test_guard();
        let mut playlist = dummy_playlist();
        playlist.uris = vec![WrappedObject::new("seed".to_string())];
        playlist.loaded_tracks = vec![LoadedTracks::dummy(WrappedObject::new("seed".to_string()))];
        playlist.loaded = true;

        let uri = playlist.add_track(track("added", "Song", 5_000)).unwrap();
        assert_eq!(playlist.get_entry_size(), 2);
        assert_eq!(playlist.get_loaded_tracks()[1].get_title(), Some("Song"));
        assert!(playlist.remove(&uri));

        let list_uri = playlist.add_track_list(list("Album", "album-uri", vec![track("x", "X", 1)])).unwrap();
        assert_eq!(list_uri.get(), "album-uri");
        assert!(playlist.get_loaded_tracks()[1].is_track_list());

        // A search-result list without a uri is refused.
        let mut search_list = list("Search", "", vec![]);
        search_list.is_search = true;
        assert!(playlist.add_track_list(search_list).is_none());
    }

    #[test]
    fn playlist_move_shifts_uri_and_loaded_tracks() {
        let _guard = test_guard();
        let mut playlist = dummy_playlist();
        playlist.uris = vec![
            WrappedObject::new("a".to_string()),
            WrappedObject::new("b".to_string()),
            WrappedObject::new("c".to_string()),
        ];
        playlist.loaded_tracks = playlist
            .uris
            .iter()
            .map(|u| LoadedTracks::dummy(u.clone()))
            .collect();
        playlist.loaded = true;

        let a = playlist.uris[0].clone();
        // Move "a" up by 1: newIndex = 0 - 1 = -1, out of bounds -> refused.
        assert!(!playlist.move_uri(&a, 1));
        // Move "c" up by 2 -> index 0.
        let c = playlist.uris[2].clone();
        assert!(playlist.move_uri(&c, 2));
        assert_eq!(playlist.uris[0].get(), "c");
        assert_eq!(playlist.get_loaded_tracks()[0].get_uri().get(), "c");
    }

    #[test]
    fn playlist_set_name_and_entry_size() {
        let _guard = test_guard();
        let mut playlist = dummy_playlist();
        playlist.set_name("mix");
        assert_eq!(playlist.get_name(), "mix");
        playlist.uris = vec![WrappedObject::new("a".to_string())];
        assert_eq!(playlist.get_entry_size(), 1);
        assert!(!playlist.is_empty());
    }

    // Playlist playing half.

    /// A playlist with two entries, each a two-track list: A1,A2 and B1,B2,
    /// plus one errored entry at the end.
    fn queue_playlist() -> Playlist {
        let mut playlist = dummy_playlist();
        playlist.uris = vec![
            WrappedObject::new("A".to_string()),
            WrappedObject::new("B".to_string()),
            WrappedObject::new("X".to_string()),
        ];
        playlist.loaded_tracks = vec![
            LoadedTracks::from_track_list(
                playlist.uris[0].clone(),
                list("A", "", vec![track("a1", "A1", 100_000), track("a2", "A2", 100_000)]),
            ),
            LoadedTracks::from_track_list(
                playlist.uris[1].clone(),
                list("B", "", vec![track("b1", "B1", 100_000), track("b2", "B2", 100_000)]),
            ),
            LoadedTracks::from_search(playlist.uris[2].clone(), SearchResultData::errored("gone")),
        ];
        playlist.loaded = true;
        playlist
    }

    #[test]
    fn first_and_last_track_pairs() {
        let _guard = test_guard();
        let playlist = queue_playlist();
        let (first_loaded, first_track) = playlist.get_first_track();
        assert!(first_loaded.is_some());
        assert_eq!(first_track.unwrap().title, "A1");
        let (last_loaded, last_track) = playlist.get_last_track();
        assert!(last_loaded.is_some());
        assert_eq!(last_track.unwrap().title, "B2");
        let empty = dummy_playlist();
        let (l, t) = empty.get_first_track();
        assert!(l.is_none() && t.is_none());
    }

    #[test]
    fn set_playable_then_calculate_next_walks_the_queue() {
        let _guard = test_guard();
        let mut playlist = queue_playlist();
        let settings = Settings::default();
        let (first_loaded, first_track) = playlist.get_first_track();
        playlist.set_playable(first_loaded.unwrap(), first_track.unwrap());
        assert!(playlist.calculate_next_with(&settings)); // first: returns the playable track
        assert_eq!(playlist.get_next().unwrap().title, "A1");
        assert!(playlist.calculate_next_with(&settings)); // advance: A1 -> A2
        assert_eq!(playlist.get_next().unwrap().title, "A2");
        assert!(playlist.calculate_next_with(&settings)); // A2 -> B1
        assert_eq!(playlist.get_next().unwrap().title, "B1");
        assert!(playlist.calculate_next_with(&settings)); // B1 -> B2
        assert_eq!(playlist.get_next().unwrap().title, "B2");
        // B2 -> end; finite settings (repeat NO) -> no wrap, queue stops. The
        // refused advance leaves the last track queued; the manager stops.
        assert!(!playlist.calculate_next_with(&settings));
        assert_eq!(playlist.get_next().unwrap().title, "B2");
    }

    #[test]
    fn repeat_playlist_wraps_to_the_start() {
        let _guard = test_guard();
        let mut playlist = queue_playlist();
        let mut settings = Settings::default();
        settings.set_repeat(Repeat::Playlist);
        let (loaded, t) = playlist.get_first_track();
        playlist.set_playable(loaded.unwrap(), t.unwrap());
        assert!(playlist.calculate_next_with(&settings));
        assert_eq!(playlist.get_next().unwrap().title, "A1");
        for expected in ["A2", "B1", "B2"] {
            assert!(playlist.calculate_next_with(&settings));
            assert_eq!(playlist.get_next().unwrap().title, expected);
        }
        // B2 -> wrap to the first entry of the playlist.
        assert!(playlist.calculate_next_with(&settings));
        assert_eq!(playlist.get_next().unwrap().title, "A1");
    }

    #[test]
    fn single_repeat_returns_the_same_track_every_time() {
        let _guard = test_guard();
        let mut playlist = queue_playlist();
        let mut settings = Settings::default();
        settings.set_repeat(Repeat::Single);
        let (loaded, t) = playlist.get_first_track();
        playlist.set_playable(loaded.unwrap(), t.unwrap());
        assert!(playlist.calculate_next_with(&settings));
        assert_eq!(playlist.get_next().unwrap().title, "A1");
        for _ in 0..3 {
            assert!(playlist.calculate_next_with(&settings));
            assert_eq!(playlist.get_next().unwrap().title, "A1");
        }
    }

    #[test]
    fn skip_walks_forward_and_backward_over_errors() {
        let _guard = test_guard();
        let mut playlist = queue_playlist();
        let settings = Settings::default();
        let (loaded, t) = playlist.get_first_track();
        playlist.set_playable(loaded.unwrap(), t.unwrap());
        assert!(playlist.calculate_next_with(&settings)); // A1
        assert!(playlist.skip(Skip::Forward, &settings)); // -> A2, and sets first
        assert_eq!(playlist.get_next().unwrap().title, "A2");
        assert!(playlist.calculate_next_with(&settings)); // first -> return A2
        assert_eq!(playlist.get_next().unwrap().title, "A2");
        // Backwards from A2 goes to A1.
        assert!(playlist.skip(Skip::Previous, &settings));
        assert_eq!(playlist.get_next().unwrap().title, "A1");
        // From A1 backwards, infinite settings wrap to the last track (B2).
        let mut infinite = Settings::default();
        infinite.set_repeat(Repeat::Playlist);
        assert!(playlist.skip(Skip::Previous, &infinite));
        assert_eq!(playlist.get_next().unwrap().title, "B2");
    }

    #[test]
    fn shuffle_picks_a_deterministic_seeded_track() {
        let _guard = test_guard();
        let mut playlist = queue_playlist();
        let mut settings = Settings::default();
        settings.set_shuffle(true);
        playlist.shuffle_seed = Some(42);
        let (loaded, t) = playlist.get_first_track();
        playlist.set_playable(loaded.unwrap(), t.unwrap());
        // first flag short-circuits shuffle: the playable track plays first.
        assert!(playlist.calculate_next_with(&settings));
        assert_eq!(playlist.get_next().unwrap().title, "A1");
        let first = playlist.calculate_next_with(&settings);
        assert!(first);
        let pick = playlist.get_next().unwrap().title.clone();
        // Same seed -> same pick (deterministic shuffler).
        playlist.shuffle_seed = Some(42);
        assert!(playlist.calculate_next_with(&settings));
        assert_eq!(playlist.get_next().unwrap().title, pick);
    }

    #[test]
    fn set_stopable_clears_the_queue() {
        let _guard = test_guard();
        let mut playlist = queue_playlist();
        let settings = Settings::default();
        let (loaded, t) = playlist.get_first_track();
        playlist.set_playable(loaded.unwrap(), t.unwrap());
        playlist.set_stopable();
        assert!(!playlist.calculate_next_with(&settings));
        assert!(playlist.get_next().is_none());
    }

    // TrackManager + player state.

    #[test]
    fn track_manager_plays_advances_and_stops() {
        let _guard = test_guard();
        let mut playlist = queue_playlist();
        let (loaded, first) = playlist.get_first_track();
        playlist.set_playable(loaded.unwrap(), first.unwrap());
        let mut track_manager = TrackManager::new();
        track_manager.set_track_queue(Some(Box::new(playlist)));

        // start(): unpauses and plays the first track.
        track_manager.start();
        assert!(!track_manager.is_paused());
        assert_eq!(track_manager.get_current_track(0).unwrap().get_title(), "A1");

        // Track end (FINISHED) advances: A1 -> A2.
        track_manager.on_track_end(TrackEndReason::Finished);
        assert_eq!(track_manager.get_current_track(0).unwrap().get_title(), "A2");

        // Track end (LOAD_FAILED) also advances: A2 -> B1.
        track_manager.on_track_end(TrackEndReason::LoadFailed);
        assert_eq!(track_manager.get_current_track(0).unwrap().get_title(), "B1");

        // Other end reasons do not advance.
        track_manager.on_track_end(TrackEndReason::Other);
        assert_eq!(track_manager.get_current_track(0).unwrap().get_title(), "B1");

        // stop() clears the playing track and the queue.
        track_manager.stop();
        assert!(track_manager.get_current_track(0).is_none());
    }

    #[test]
    fn track_manager_without_queue_stops_on_skip() {
        let _guard = test_guard();
        let mut track_manager = TrackManager::new();
        track_manager.set_track_queue(None);
        track_manager.skip(); // queueWrapper null -> stop()
        assert!(track_manager.get_current_track(0).is_none());
        // start() with no queue: unpause, then skip -> stop.
        track_manager.start();
        assert!(!track_manager.is_paused());
        assert!(track_manager.get_current_track(0).is_none());
    }

    #[test]
    fn audio_player_start_track_respects_no_interrupt() {
        let _guard = test_guard();
        let mut audio = AudioPlayerState::default();
        assert!(audio.start_track(track("a", "One", 1), false));
        assert!(!audio.start_track(track("b", "Two", 2), true)); // noInterrupt refuses
        assert_eq!(audio.get_playing_track().unwrap().title, "One");
        assert!(audio.start_track(track("b", "Two", 2), false)); // interrupt replaces
        assert_eq!(audio.get_playing_track().unwrap().title, "Two");
        audio.stop_track();
        assert!(audio.get_playing_track().is_none());
    }

    #[test]
    fn playing_track_position_clamps_and_seeks_only_seekable() {
        let _guard = test_guard();
        let mut player_state = MusicPlayerState::new();
        let mut track_manager = TrackManager::new();

        let mut song = track("a", "One", 100_000);
        song.position_ms = 999_999;
        track_manager.audio_mut().start_track(song, false);
        // The view reads the live player position, not the stored track
        // position: 999_999 clamps to the 100s duration.
        let view = track_manager.get_current_track(999_999).unwrap();
        assert_eq!(view.get_position(), 100_000); // clamped to duration

        track_manager.set_current_position(42_000, &mut player_state.current_track_position);
        let view = track_manager.get_current_track(42_000).unwrap();
        assert_eq!(view.get_position(), 42_000);

        // Streams are not seekable.
        let mut stream = TrackData::new("s", "Live", 0, true);
        stream.position_ms = 5000;
        track_manager.audio_mut().start_track(stream.clone(), false);
        track_manager.set_current_position(1_000, &mut player_state.current_track_position);
        assert_eq!(track_manager.get_current_track(0).unwrap().get_position(), 5000);
    }

    #[test]
    fn player_speed_pitch_clamped_and_volume_applied() {
        let _guard = test_guard();
        let mut player = MusicPlayerState::new();
        player.set_speed(50.0);
        assert_eq!(player.get_speed(), 10.0);
        player.set_speed(0.01);
        assert_eq!(player.get_speed(), 0.05);
        player.set_pitch(20.0);
        assert_eq!(player.get_pitch(), 10.0);
        player.set_volume(55);
        assert_eq!(player.get_volume(), 55);
        // The stand-in audio player takes its volume directly; the player ->
        // audio wiring is the render half's job.
        manager::track_manager_lock().lock().unwrap().audio_mut().set_volume(30);
        assert_eq!(manager::track_manager_lock().lock().unwrap().audio().get_volume(), 30);
    }

    // MusicPlayerUtils via the global singletons.

    #[test]
    fn skip_back_restarts_early_positions_instead_of_skipping() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!("mp-globals-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        manager::setup(true, &dir);

        let playlist = queue_playlist();
        manager::with_playlists(|playlists| {
            playlists.add(playlist);
            if let Some(added) = playlists.get_mut(playlists.size() - 1) {
                let (loaded, t) = added.get_first_track();
                added.set_playable(loaded.unwrap(), t.unwrap());
                let snapshot = added.clone();
                playlists.set_playing(&snapshot);
            }
        })
        .unwrap();

        // Queue it and play A1.
        {
            let tm = manager::track_manager_lock();
            let mut tm = tm.lock().unwrap();
            tm.set_track_queue(Some(manager::playing_queue()));
            tm.start();
        }
        assert_eq!(
            {
                let tm = manager::track_manager_lock();
                tm.lock().unwrap().get_current_track(0).unwrap().get_title().to_string()
            },
            "A1"
        );

        // A1 sits at 50 seconds of a 100 second track: max duration window is
        // 10s (duration/10 capped at 10s), so 50s restarts the track.
        {
            let tm = manager::track_manager_lock();
            let mut tm = tm.lock().unwrap();
            let player = manager::player_lock();
            let mut player = player.lock().unwrap();
            tm.set_current_position(50_000, &mut player.current_track_position);
        }
        skip_back();
        {
            let tm = manager::track_manager_lock();
            let tm = tm.lock().unwrap();
            let view = tm.get_current_track(0).unwrap();
            assert_eq!(view.get_title(), "A1");
            assert_eq!(view.get_position(), 0);
        }

        // Early position (2s) goes to the previous track instead. With the
        // default finite settings the wrap is refused, so switch to repeat
        // PLAYLIST first: the previous track of A1 is B2.
        manager::with_settings(|settings| settings.set_repeat(Repeat::Playlist)).unwrap();
        {
            let tm = manager::track_manager_lock();
            let mut tm = tm.lock().unwrap();
            let player = manager::player_lock();
            let mut player = player.lock().unwrap();
            tm.set_current_position(2_000, &mut player.current_track_position);
        }
        skip_back();
        let tm = manager::track_manager_lock();
        let tm = tm.lock().unwrap();
        assert_eq!(tm.get_current_track(0).unwrap().get_title(), "B2");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skip_forward_advances_and_stop_on_end() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!("mp-globals2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        manager::setup(true, &dir);

        let playlist = queue_playlist();
        manager::with_playlists(|playlists| {
            playlists.add(playlist);
            if let Some(added) = playlists.get_mut(playlists.size() - 1) {
                let (loaded, t) = added.get_first_track();
                added.set_playable(loaded.unwrap(), t.unwrap());
                let snapshot = added.clone();
                playlists.set_playing(&snapshot);
            }
        })
        .unwrap();

        {
            let tm = manager::track_manager_lock();
            let mut tm = tm.lock().unwrap();
            tm.set_track_queue(Some(manager::playing_queue()));
            tm.start();
        }

        for expected in ["A2", "B1", "B2"] {
            skip_forward();
            let tm = manager::track_manager_lock();
            let tm = tm.lock().unwrap();
            assert_eq!(tm.get_current_track(0).unwrap().get_title(), expected);
        }
        // The final forward passes the end; finite settings refuse the skip,
        // so B2 keeps playing.
        skip_forward();
        let tm = manager::track_manager_lock();
        let tm = tm.lock().unwrap();
        assert_eq!(tm.get_current_track(0).unwrap().get_title(), "B2");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // PlaylistManager (gzip file).

    #[test]
    fn playlist_manager_round_trips_a_gzip_file() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!("mp-pm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut manager = PlaylistManager::new();
        manager.set_base_path(&dir);
        manager.load_from_file();
        assert!(dir.join("playlist.json.gz").exists());

        let playlists = manager.get_playlists_mut().unwrap();
        let mut playlist = Playlist::new("mix");
        playlist.uris = vec![WrappedObject::new("a".to_string())];
        playlists.add(playlist);
        manager.write_to_file();

        let mut reloaded = PlaylistManager::new();
        reloaded.set_base_path(&dir);
        reloaded.load_from_file();
        let playlists = reloaded.get_playlists().unwrap();
        assert_eq!(playlists.size(), 1);
        assert_eq!(playlists.get(0).unwrap().get_name(), "mix");
        assert_eq!(playlists.get(0).unwrap().get_entry_size(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn playlist_manager_corrupted_file_falls_back_to_new_playlists() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!("mp-pm2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("playlist.json.gz"), b"not gzip at all").unwrap();
        let mut manager = PlaylistManager::new();
        manager.set_base_path(&dir);
        manager.load_from_file();
        assert!(manager.get_playlists().is_some());
        assert_eq!(manager.get_playlists().unwrap().size(), 0);

        // Valid gzip with garbage inside: the same ZipException branch.
        let file = std::fs::File::create(dir.join("playlist.json.gz")).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, b"{{{not json").unwrap();
        encoder.finish().unwrap();
        let mut manager = PlaylistManager::new();
        manager.set_base_path(&dir);
        manager.load_from_file();
        assert!(manager.get_playlists().is_some());
        assert_eq!(manager.get_playlists().unwrap().size(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[should_panic(expected = "malformed musicplayer.json")]
    fn client_config_malformed_json_panics_like_the_uncaught_exception() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!("mp-cfg3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("musicplayer.json"), "{nope").unwrap();
        let _ = ClientConfig::load(&dir.join("musicplayer.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[should_panic(expected = "malformed settings.json")]
    fn settings_manager_malformed_json_panics_like_the_uncaught_exception() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!("mp-sm3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "{nope").unwrap();
        let mut manager = SettingsManager::new();
        manager.set_base_path(&dir);
        manager.load_from_file();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn playlists_deserialization_assigns_fresh_identities() {
        let _guard = test_guard();
        let json = r#"{"playlists":[{"name":"one","uris":[{"o":"a"}]},{"name":"two","uris":[]}]}"#;
        let playlists: Playlists = serde_json::from_str(json).unwrap();
        assert_eq!(playlists.size(), 2);
        assert_ne!(playlists.get(0).unwrap().get_id(), playlists.get(1).unwrap().get_id());
        assert_eq!(playlists.get(0).unwrap().get_entry_size(), 1);
    }
}
