# horizon-music-player

Native Horizon (Rust Minecraft client) port of the
[Music Player](https://github.com/MC-U-Team/Music-Player) mod by U-Team.

Upstream is Apache-2.0 (c) 2024 U-Team. This port keeps the upstream license
and credits; the runtime logic is ported 1:1.

## Status (slice 1)

- Full runtime: track data model, playlist/queue logic (next, back, shuffle,
  repeat variants), track manager playback state, search-result/playlist loading.
- 41/41 tests green (`cargo test`).
- Documented degradation: the audio engine (lavaplayer-backed playback, YouTube/
  SoundCloud sources, PCM output) is not ported yet; a Horizon native mod has no
  audio decode/streaming surface yet. Queue, playlist and manager logic run 1:1
  against a pluggable search backend.
