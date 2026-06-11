# `.1` — IIDX Chart Format

A `.1` file holds all of a song's charts (one per difficulty). For audio reconstruction we
only need the **sounding events**: which keysound (a 1-based sample number indexing the `.s3p`;
see `S3P_FORMAT.md`) plays at which millisecond. Any single difficulty reconstructs the *same*
song — a sound that is auto-played in an easy chart becomes a player-hit note in a hard one,
but the union of sounds is identical.

All integers are little-endian.

## Header — difficulty slot table

```
offset  type        field
0x00    slot[14]    14 slots, 8 bytes each:
                        u32 offset   file offset of this difficulty's event stream
                        u32 length   byte length of the stream (0 = difficulty absent)
```

A non-empty slot is one difficulty. The slot→difficulty map (confirmed by cross-referencing
each populated slot against `music_data.bin` levels for songs 30000 / 31000 / 32000):

| slot | difficulty | slot | difficulty |
|---|---|---|---|
| 0 | SP NORMAL | 6 | DP NORMAL |
| 1 | SP HYPER | 7 | DP HYPER |
| 2 | SP ANOTHER | 8 | DP ANOTHER |
| 3 | SP BEGINNER | 9 | DP BEGINNER |
| 4 | SP LEGGENDARIA | 10 | DP LEGGENDARIA |

Slots 5, 11, 12, 13 are unused / not yet identified. **SP NORMAL (slot 0) exists for every
song**, which makes it the safe default. Slots 0/1/2 and 6/7/8 are directly confirmed; beginner
and leggendaria are placed per the known bm2dx layout.

## Events

Each event is 8 bytes:

```
offset  type   field
+0x00   u32    time_ms    event time in milliseconds
+0x04   u8     type       event kind (table below)
+0x05   u8     param      lane index (for note / assign events)
+0x06   u16    value      sample number, or type-specific payload
```

Events within a slot are ordered by time.

| type | meaning | param | value |
|---|---|---|---|
| 0 | P1 visible note on lane `param` | lane | usually 0 — sound comes from the lane's assigned sample |
| 1 | P2 visible note (DP) | lane | as type 0 |
| 2 | assign a keysound to a lane (for upcoming notes on it) | lane | 1-based sample number |
| 4 | bar line | — | — |
| 5 | BPM change | — | tempo payload |
| 6 | end of chart | — | `time_ms` = `0x7FFFFFFF` |
| 7 | **auto-play** a keysound at `time_ms` | — | 1-based sample number |
| 8 | initial lane→sample assignment (song start) | lane | 1-based sample number |
| 12 | metadata (e.g. note counts) | — | — |
| 16 | metadata | — | — |

## Keysound / lane mechanism

IIDX is keysound-driven (BMS-style): there is no pre-mixed song, only samples plus a schedule.

- **Auto-play (type 7)** plays `value` at `time_ms` directly.
- **Visible notes (type 0/1)** carry no sample of their own. An **assignment** (type 8 at the
  start, or type 2 mid-song) binds a sample to a *lane*; the next visible note on that lane
  plays whatever sample is currently bound. To resolve a note's sound, walk events in order,
  keep a `lane -> sample` table, and read it when a note fires.

## Reconstructing the song

1. Pick a difficulty (default SP NORMAL = slot 0).
2. Walk its events in order, maintaining `lane -> sample`.
3. Emit a **sounding** `(time_ms, sample)` for every type 7 (auto-play) and every type 0/1
   note (using the lane's current sample). Ignore 4/5/6/12/16.
4. Decode each referenced sample to PCM and mix every sounding onto a timeline at `time_ms`.
   Song length = the largest real `time_ms` (ignore the `0x7FFFFFFF` end sentinel), extended
   to cover the tail of the last sample.

## Measured example — 30000, SP NORMAL (slot 0)

- 4423 events, duration 114650 ms (~114.65 s).
- Event-type histogram: `{0: 1081, 2: 890, 4: 1, 5: 1, 6: 1, 7: 2369, 8: 6, 12: 72, 16: 2}`.
- Sample 1 (the background base) is auto-played by a type-7 event at `t = 0`.
- Sounding events = 2369 auto-plays + the visible notes that have an assigned lane sample.
