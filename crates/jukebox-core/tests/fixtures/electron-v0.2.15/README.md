# Electron v0.2.15 data directory

A data directory written by the Electron build itself, used to prove the Rust build reads
what Electron leaves behind, on every platform CI runs.

How it was made:

1. The v0.2.15 Electron app was started with `--user-data-dir` pointing at an empty folder,
   holding only a `config.json` (port 8094, admin password `fixture-admin`).
2. Its library was four generated test tones — tagged FLAC and MP3 with shared embedded
   cover art, an MP3 by an artist with no album, and an untagged Opus file named `Löyly.opus`.
3. Through its own HTTP API: a library scan, three songs queued, a downvote, a standby
   playlist entry, two bans (one timed, one permanent) and a skip. Every table except `meta`
   (which the app never writes) ends up with rows Electron wrote.
4. The app was stopped cleanly. Machine-specific values were then replaced: the music folder
   became `/fixtures/music` (in `config.json`, rewritten with the same `JSON.stringify` call
   config.ts uses, and in `tracks.path`), and hostnames became `guest-laptop`. The database
   was vacuumed so the replaced values don't linger in free pages.

Nothing else was edited: the schema, the rows, the column types and the file formats are
Electron's.
