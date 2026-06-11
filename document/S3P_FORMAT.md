# `.s3p` — IIDX Keysound Archive Format

`.s3p` (magic `S3P0`) is the container holding a beatmania IIDX song's **keysounds**: the
hundreds-to-thousands of short audio samples that a chart triggers to play the song. There is
one `.s3p` per song, holding every sample that song needs. The chart (`.1`) references samples
by index; the song is reconstructed by mixing them onto a timeline (see `CHART_1_FORMAT.md`).

All integers are little-endian.

## Container layout

```
offset  type           field
0x00    char[4]        magic = "S3P0"
0x04    u32            count          number of entries (keysounds)
0x08    entry[count]   entry table    8 bytes each:
                           u32 offset     absolute file offset of the S3V0 block
                           u32 size       total block size (header + payload)
```

Each entry points at an **S3V0 block**:

```
offset (within block)   type      field
+0x00                   char[4]   magic = "S3V0"
+0x04                   u32       header_size = 0x20
+0x08                   u32       data_size   = size - 0x20   (payload length)
+0x0C .. +0x20          ...       rest of header (track id / volume etc.; not needed to decode)
+0x20                   u8[]      payload = one ASF/WMA keysound (data_size bytes)
```

So a keysound's raw audio bytes are `file[offset + 0x20 .. offset + size]`.

## Sample indexing

Charts reference keysounds by a **1-based** sample number `N`, which maps to entry index
`N - 1` (sample 1 = entry 0). Entry 0 (sample 1) is consistently the **largest** entry: it is
the song's **background layer** (the long bed of drums/bass/ambience), auto-played once at
`t = 0`. Every other entry is a short one-shot keysound.

## Payload codec (measured)

The payload is an **ASF container** — header-object GUID `75B22630-668E-11CF-A6D9-00AA0062CE6C`,
stored as the bytes `30 26 B2 75 8E 66 CF 11 A6 D9 00 AA 00 62 CE 6C` — wrapping **WMA
Standard v2**. Parsing the `WAVEFORMATEX` in the ASF *Stream Properties* object for every
keysound of three sample songs (30000 / 31000 / 32000) yields a perfectly uniform result:

| WAVEFORMATEX field | value |
|---|---|
| wFormatTag | `0x0161` (WMA v2) |
| nChannels | 2 (stereo) |
| nSamplesPerSec | 44100 |
| wBitsPerSample | 16 |

> **Correction.** Although `.s3p` shares the ASF container with SDVX's `.s3v`, IIDX keysounds
> are **WMA v2 (`0x0161`)**, *not* WMA Pro (`0x0162`). A decoder built only for WMA Pro will
> not decode them — a WMA Standard (`wmav2`) decoder is required. (Earlier project notes that
> called this "same as `.s3v` / WMA Pro" were wrong about the codec, right about the container.)

## Measured archives

| song | entries | notes |
|---|---|---|
| 30000 | 1186 | base (entry 0) ≈ 4.88 MB; median keysound ≈ 17 KB |
| 31000 | 1681 | uniform WMA v2 |
| 32000 | 752 | uniform WMA v2 |

All three are 100% WMA v2 / stereo / 44100 Hz / 16-bit, with no per-keysound variation.

## Practical notes for unpacking / decoding

- **Unpack:** trust the entry table; for each entry skip the 0x20 S3V0 header and take
  `data_size` bytes. (A whole-file magic scan for `S3V0` also works if a table looks suspect.)
- **Decode:** feed each payload to a `wmav2` decoder. Because every keysound is 44100 Hz
  stereo, a renderer can mix at 44.1 kHz natively and resample once at the very end, rather
  than resampling every keysound.
- **Cache:** decode each unique sample once — a chart triggers most samples many times.

## Where `.s3p` lives in the game

It ships either loose (`sound/<id5>/<id5>.s3p`, version 30+) or packed inside a per-song
`.ifs` (older versions); the S3P0 container is identical either way. Alongside it sit a
`<id5>_pre.2dx` preview and the `<id5>.1` chart.
