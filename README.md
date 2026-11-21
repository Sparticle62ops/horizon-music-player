# horizon-music-player

Music Player (MC-U-Team/Music-Player, Apache-2.0 (c) 2024 U-Team) ported to
Horizon as a native Rust mod. Exact 1:1 behavior port, commit `1120a0c`
("Add new changelog", 2026-10-25). Modrinth: https://modrinth.com/mod/music-player
(278,370 downloads).

## This build (v1.0.0, slice 1 on the mod API)

Rebuilt on the current SDK (api 0.2, horizon `f00125f61f794aaa3d4e950d301f353b35b6ee3d`,
mixins `e59da9094f0c2fdda6c627fe9eff22dec6a906f0`): `runtime.rs` is the 1:1
slice-1 core (client config, settings + managers, playlists with the
playing-queue state machine, the track manager over a stand-in audio player,
the skip utility), and `src/lib.rs` is the mod API wiring:

- the original's settings registered on Horizon's Options > Mods page with
  lang labels: volume (0..100 step 1, default 10), repeat (no / playlist /
  single, default no), shuffle (off), show ingame overlay (on), show ingame
  menu overlay (off), ingame overlay position (up left / up right /
  down right / down left, default down right), key works in GUI (on),
  internal playlists (off) — same defaults as upstream's settings.json;
- `client_setup` runs the mod entry 1:1: load
  `config/musicplayer/musicplayer.json` (defaults when missing or broken),
  set up the data directory (internal playlists: `config/musicplayer`,
  otherwise `~/.config/musicplayer`, same as `MusicPlayerFiles.load`), load
  `settings.json`, then sync the Horizon settings on top (setters mark dirty
  and the manager flushes back to `settings.json`).

41 tests green (`cargo test`), covering the config, settings, playlists,
queue state machine, track manager and skip logic.

## Known degradations (upstream gaps on Horizon)

- No audio decode/streaming surface exists on a Horizon native mod yet
  (upstream uses lavaplayer with youtube/soundcloud sources); the port models
  the `IAudioTrack` / `IAudioTrackList` / `ISearchResult` data and drives the
  queue state machine over a stand-in audio player. A follow-up issue will
  ask for an audio decode/streaming API surface.
- No GUI screens (upstream's menu screens and the ingame overlay); the
  GUI-layer hook is tracked in horizon-rust/horizon #6243.
- No keybind hooks for the play/pause/skip keys yet.

## License

Apache-2.0. Upstream: https://github.com/MC-U-Team/Music-Player
