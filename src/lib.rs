//! Music Player for Horizon: the mod API wiring of the slice-1 core.
//!
//! The mod registers the original's settings on Horizon's Options > Mods page
//! (the same fields upstream's `settings.json` holds, same defaults) and runs
//! the mod entry 1:1 on `client_setup`: load the client config
//! (`config/musicplayer/musicplayer.json`, defaults when missing or broken),
//! set up the music-player data directory the same way
//! `MusicPlayerFiles.load` picks `config/musicplayer` for internal playlists
//! or `~/.config/musicplayer` otherwise, load `settings.json`, then sync the
//! Horizon settings on top (each setter marks the settings dirty and the
//! manager flushes them back to `settings.json`).
//!
//! Slice 1 scope: the core non-GUI logic carries 1:1 in `runtime` (the client
//! config, settings + managers, playlists with the playing-queue state
//! machine, the track manager over a stand-in audio player, and the skip
//! utility), 41 tests green. The audio decode and streaming layer
//! (lavaplayer, youtube/soundcloud sources) and the GUI screens are later
//! slices and documented degradations: a Horizon native mod has no audio
//! decode/streaming surface yet, and the screens wait on the GUI-layer hooks.
#[path = "../runtime.rs"] pub mod runtime;

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use mod_api::{ClientSetupEvent, ClientHost, RegistrationContext, Registrar, SettingSpec};

/// The game's data root (upstream: `FabricLoader.getInstance().getGameDir()`).
pub fn game_dir() -> PathBuf {
    std::env::var_os("MUSIC_PLAYER_GAME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `config/musicplayer/musicplayer.json`.
pub fn config_path() -> PathBuf {
    game_dir().join("config").join("musicplayer").join("musicplayer.json")
}

/// The live music-player state the later slices build on: the data-directory
/// setup and the settings manager (upstream: the MusicPlayer singleton's
/// files + settings manager fields).
struct Live {
    files: runtime::MusicPlayerFiles,
    settings: runtime::SettingsManager,
}

static LIVE: OnceLock<Mutex<Live>> = OnceLock::new();

pub fn set_live(files: runtime::MusicPlayerFiles, settings: runtime::SettingsManager) {
    let cell = LIVE.get_or_init(|| Mutex::new(Live {
        files: runtime::MusicPlayerFiles::new(),
        settings: runtime::SettingsManager::new(),
    }));
    *cell.lock().expect("the live cell is not poisoned") = Live { files, settings };
}

fn repeat_from_index(index: u32) -> runtime::Repeat {
    match index {
        0 => runtime::Repeat::No,
        1 => runtime::Repeat::Playlist,
        _ => runtime::Repeat::Single,
    }
}

fn overlay_position_from_index(index: u32) -> runtime::IngameOverlayPosition {
    match index {
        0 => runtime::IngameOverlayPosition::UpLeft,
        1 => runtime::IngameOverlayPosition::UpRight,
        2 => runtime::IngameOverlayPosition::DownRight,
        _ => runtime::IngameOverlayPosition::DownLeft,
    }
}

#[derive(Default)]
struct Mod;

#[mod_api::horizon_mod]
impl mod_api::Hooks for Mod {
    /// The original's settings as settings, same defaults.
    fn register(&self, registrar: &mut dyn Registrar, _context: &RegistrationContext) {
        let _ = registrar.register_setting(SettingSpec::range(
            "volume",
            "setting.musicplayer.volume",
            0.0,
            100.0,
            1.0,
            10.0,
        ));
        let _ = registrar.register_setting(SettingSpec::choice(
            "repeat",
            "setting.musicplayer.repeat",
            [
                "option.musicplayer.repeat.no",
                "option.musicplayer.repeat.playlist",
                "option.musicplayer.repeat.single",
            ],
            0,
        ));
        let _ = registrar.register_setting(SettingSpec::toggle(
            "shuffle",
            "setting.musicplayer.shuffle",
            false,
        ));
        let _ = registrar.register_setting(SettingSpec::toggle(
            "show_ingame_overlay",
            "setting.musicplayer.show_ingame_overlay",
            true,
        ));
        let _ = registrar.register_setting(SettingSpec::toggle(
            "show_ingame_menue_overlay",
            "setting.musicplayer.show_ingame_menue_overlay",
            false,
        ));
        let _ = registrar.register_setting(SettingSpec::choice(
            "ingame_overlay_position",
            "setting.musicplayer.ingame_overlay_position",
            [
                "option.musicplayer.overlay_position.up_left",
                "option.musicplayer.overlay_position.up_right",
                "option.musicplayer.overlay_position.down_right",
                "option.musicplayer.overlay_position.down_left",
            ],
            2,
        ));
        let _ = registrar.register_setting(SettingSpec::toggle(
            "key_work_in_gui",
            "setting.musicplayer.key_work_in_gui",
            true,
        ));
        let _ = registrar.register_setting(SettingSpec::toggle(
            "internal_playlists",
            "setting.musicplayer.internal_playlists",
            false,
        ));
    }

    /// 1:1 of the original's mod entry: client config, data directory,
    /// settings.json, then the Horizon settings synced on top.
    fn client_setup(&self, host: &mut dyn ClientHost, _event: &mut ClientSetupEvent) {
        let mut config = std::fs::read_to_string(config_path())
            .ok()
            .and_then(|text| serde_json::from_str::<runtime::ClientConfig>(&text).ok())
            .unwrap_or_default();
        if let Some(value) = host.setting_bool("internal_playlists") {
            config.internal_playlists = value;
        }
        let mut files = runtime::MusicPlayerFiles::new();
        files.load(config.internal_playlists, &game_dir());
        let data_dir = files
            .get_directory()
            .map(|path| path.to_path_buf())
            .unwrap_or_else(|| game_dir().join("config").join("musicplayer"));
        let mut settings_manager = runtime::SettingsManager::new();
        settings_manager.set_base_path(&data_dir);
        settings_manager.load_from_file();
        if let Some(settings) = settings_manager.get_settings_mut() {
            if let Some(value) = host.setting_f64("volume") {
                settings.set_volume(value.round() as i32);
            }
            if let Some(index) = host.setting_choice("repeat") {
                settings.set_repeat(repeat_from_index(index));
            }
            if let Some(value) = host.setting_bool("shuffle") {
                settings.set_shuffle(value);
            }
            if let Some(value) = host.setting_bool("show_ingame_overlay") {
                settings.set_show_ingame_overlay(value);
            }
            if let Some(value) = host.setting_bool("show_ingame_menue_overlay") {
                settings.set_show_ingame_menue_overlay(value);
            }
            if let Some(index) = host.setting_choice("ingame_overlay_position") {
                settings.set_ingame_overlay_position(overlay_position_from_index(index));
            }
            if let Some(value) = host.setting_bool("key_work_in_gui") {
                settings.set_key_work_in_gui(value);
            }
        }
        settings_manager.write_to_file();
        set_live(files, settings_manager);
    }
}
