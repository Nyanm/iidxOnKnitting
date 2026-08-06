# IIDX on Knitting

## 简介

IIDX on Knitting 是一款 BMS 风格的音游音频重建器：把谱面里的 key 音按时间轴拼回去，混成完整的歌曲，导出为 Opus 编码的 Ogg。

程序静态链接了裁剪版 FFmpeg，并自带一个极简 IFS/KBIN 解析器与一个 Konami 4-bit ADPCM 解码器，一站式完成从容器解包、解码、PCM 混音、母带处理到 Opus 编码与 Ogg 封装的全流程。**无需系统 FFmpeg。**

当前支持三个游戏分支：

| 分支           | 需要重建什么                        | 输入                                | 覆盖范围          |
|--------------|-------------------------------|-----------------------------------|---------------|
| **IIDX**     | 整首歌都由 key 音拼成                 | 谱面 `.1` + 键音库 `.s3p` / `.2dx`     | 初代 ~ 33 代全部歌曲 |
| **GITADORA** | 鼓 / 吉他 / 贝斯由 key 音拼成，叠在预混伴奏底上 | `m<id>_seq.ifs` + `m<id>_bgm.ifs` | FUZZ-UP 全部歌曲  |
| **SDVX**     | 无需重建，歌曲本身就是成品文件               | `.s3v`，或 `.2dx` 容器                | 单文件转码         |

本程序不包含任何所属 ©Konami Arcade Games 版权所有的信息。

## 编译

```
cargo build -r
```

`/vendor` 内已存放编译好的裁剪版 FFmpeg 静态库，无需再次编译。初次 clean 构建依赖 LLVM + VS 2022 MSVC 工具链，增量构建不需要。

## 使用

命令分两层，第一层是游戏，第二层是操作：

```
iidxOnKnitting iidx render  --ifs <.ifs>                       -o out.ogg  [-d spa]
iidxOnKnitting iidx render  --audio <.s3p|.2dx> --chart <.1>   -o out.ogg  [-d spa]
iidxOnKnitting iidx convert <file>                             -o out.ogg
iidxOnKnitting gd   render  --seq <_seq.ifs> --bgm <_bgm.ifs>  -o out.ogg
iidxOnKnitting sdvx convert <file>                             -o out.ogg
```

`render` 重建歌曲（谱面 + key 音），`convert` 原样转码一个已经混好的音频文件。

## 代码结构

```
src/audio/      通用音频处理，不知道任何游戏的存在（ADPCM 解码 / 混音时间轴 / 母带）
src/iidx/       chart.rs 谱面 · song.rs 组装
src/gitadora/   bmp.rs 伴奏底 · va3.rs 键音库 · sq3.rs 谱面 · song.rs 组装
src/unpack.rs   S3P0（IIDX）与 2DX9（IIDX + SDVX 共用）容器
src/codec.rs    libav FFI：解码与 Opus 编码
src/render.rs   机制层：每个入口只做一件事，无法代替调用方决定的情况用类型报出来
src/run.rs      策略层：run_iidx / run_gitadora / run_sdvx，一个游戏一个调用
src/main.rs     只做参数解析，每个叶子命令对应一个 run_*
```

### 作为库调用

对外有两层入口，嵌入方几乎总该用第一层：

```rust
use iidx_on_knitting::{run_iidx, run_gitadora, run_sdvx, IidxSource};

run_sdvx(&path_s3v, &path_out)?;                                   // .s3v 与 .2dx 容器都行
run_gitadora(&path_seq_ifs, &path_bgm_ifs, &path_out)?;
run_iidx(IidxSource::Packed { ifs: &path_ifs, difficulty }, &path_out)?;
run_iidx(IidxSource::Loose { audio: &a, chart: &c, difficulty }, &path_out)?;
run_iidx(IidxSource::PreMixed(&path_pre_2dx), &path_out)?;         // 只转码
```

`run_*` 返回普通的 `anyhow::Result<()>`，并且**自带那两条只有唯一合理答案的重试规则**：键音库其实是单个成品音频时改为转码；当作裸音频传入的文件其实是 2DX9 容器时改走容器解包。两者都在解码或写盘之前靠容器 magic 判定，重试不花代价 —— 这属于本库该负责的策略，不该让每个调用方各写一遍。

第二层是 `render_*` / `convert_*`，每个只做一件事，把上述两种情况作为 `RenderError` 的类型化分支报出来，供需要自己分流的调用方使用。

---

## 分支：IIDX

一首歌是一个谱面 `.1` 加一个键音库（`.s3p` 或 `.2dx`），两者可能打包在同一个 `.ifs` 里，也可能是散装文件。谱面的每个音符点名一个 key 音，整首歌就是这些 key 音的和 —— 没有伴奏底。因为没有预混母带垫在下面，混出来的绝对电平是任意的，所以只做峰值归一化（仅当峰值超过满刻度时按比例缩回）。

`-d` 选择读取哪个难度的事件流。任一难度都能重建出同一首歌，但不是每首歌都有每个难度 —— `spn` 是唯一保证存在的。

> **多音源曲的取舍**：极少数早期曲在一个文件夹 / `.ifs` 内含多个键音 `.2dx`（本库实测约 68 首，多为新旧两版，或按难度分的音源）。此时程序按 `<id>a` → `<id>1` → `<id>` 的优先级**自动选其一**渲染 —— `<id>a` 通常是现代复活 / 重做版（如初代曲 01000 的 `01000a`）。**这是个近似取舍**：所选未必对每个难度都是"原配"音源，但能保证这些曲也出声。

---

## 分支：GITADORA

一首歌拆在两个 `.ifs` 里，必须同时指定：

- `m<id>_seq.ifs`：谱面（`d<id>.sq3` 鼓 / `g<id>.sq3` 吉他与贝斯）与键音库（`spu<id>d.va3` / `spu<id>g.va3`，吉他和贝斯共用后者，靠 sound_id 区间分开）
- `m<id>_bgm.ifs`：若干条预混伴奏底，文件名 `bgm<id><d|_><g|_><b|_>k.bin` 的三个字符表示**该底已经含有哪些声部**

程序挑选"烘进去的声部最少"的那条底，只拼接它缺失的声部 —— 已在底里的声部绝不重复叠加。绝大多数曲存在纯伴奏底 `___k`，于是鼓、吉他、贝斯全部由 key 音重建；少数曲只有含贝斯的 `__bk`，则贝斯沿用游戏母带。

GITADORA 的难度不影响音频（同声部各难度的 note 集合会被合并去重），因此没有 `-d` 参数。原生 48 kHz，全程无重采样直通 Opus。

Konami 4-bit ADPCM 不在 FFmpeg 中，由本项目自行实现于 `src/audio/adpcm.rs`，与 `vendor/` 无关。

> **响度取舍**：GITADORA 的伴奏底本身已是压满的成品母带，叠上 key 音后峰值中位数约达满刻度的 2.2 倍。程序对 key 音层施加固定 **0.70** 衰减，再过一级无状态软膝限幅（阈值 0.95）。实测成品响度中位数 −10.6 dBFS，与游戏自带预混母带基本齐平，且中位仅约 1% 的采样被限幅触碰。**代价是尾部**：约 2.3% 的曲有超过 5% 的采样被限幅，个别曲可达 17%，这些会有可听见的压缩感。若要彻底消除，需改用逐曲响度归一化。

> **不支持的曲**：实测数据包内 1479 首中可渲染 1359 首。剩下 120 首的两个 `.ifs` 都是 1024 字节的占位桩（magic 非 IFS），曲目内容并不在数据包里。另有 21 首曲的少量音符（占各曲 0.4–2.2%）引用了 0/1/2 这类空音 sound_id，那几个音不出声 —— 与 IIDX 谱面引用 reserved sample number 同类。

---

## 分支：SDVX

SDVX 不发 key 音，歌曲直接以成品文件分发，因此没有可重建的东西，只需转码：`.s3v` 是裸音频，`.2dx` 是 2DX9 容器（取第一条为主混音）。`convert` 会自行分辨两者，无需指定。

---

## 第三方代码声明

`src/tool/ifs.rs` 中关于解析 IFS 文件的代码参考了 [![ifstools](https://img.shields.io/badge/ifstools-blue)](https://github.com/mon/ifstools) 和 [![kbinxml](https://img.shields.io/badge/kbinxml-blue)](https://github.com/mon/kbinxml) 的实现。

`src/audio/adpcm.rs` 与 `src/gitadora/` 中 GITADORA 各格式的字段布局参考了 [![gitadora-customs](https://img.shields.io/badge/gitadora--customs-blue)](https://github.com/fisyher/gitadora-customs) 的实现，并逐条在真实数据上复核。该实现的若干处与本数据包实测不符，已在本项目中修正：`tick` 的单位是 1/300 **秒**而非 1/300 毫秒；`volume` 在所有归档版本下都是 0..127 线性值，不经 `VOLUME_TABLE` 换算；VA3 条目的 `filesize` 可直接采信；标记为 `is_metadata` 的谱面块仍可能携带真实 key 音。

| 组件                                                            | 版本    | 来源                        | 许可证               |
|---------------------------------------------------------------|-------|---------------------------|-------------------|
| FFmpeg (libavcodec / libavformat / libavutil / libswresample) | 8.0.2 | <https://ffmpeg.org/>     | LGPL-2.1-or-later |
| libopus                                                       | 1.5.2 | <https://opus-codec.org/> | BSD-3-Clause      |

以上静态库均为 **裁剪构建**，仅保留本工具所需的组件、未开启任何 GPL 组件：

| 类别           | 保留组件                                                    |
|--------------|---------------------------------------------------------|
| 解码器 Decoder  | `wmav1` · `wmav2` · `wmapro` · `adpcm_ms` · `pcm_s16le` |
| 解复用器 Demuxer | `asf`（s3p 内 WMA） · `wav`（2dx 内 RIFF/WAVE）               |
| 编码器 Encoder  | `libopus`                                               |
| 封装器 Muxer    | `opus`（Ogg-Opus）                                        |

用户可依据各组件原许可证获取源码、自行重编并替换 `vendor/` 中的预编译库。本项目的 MIT 许可证仅覆盖 `src/` 下的本项目源码，**不覆盖** `vendor/` 中的 FFmpeg/libopus 头文件与预编译静态库。

---

# IIDX on Knitting · English

## Introduction

IIDX on Knitting is a BMS-style renderer for rhythm-game audio: it lays a chart's keysounds back onto the timeline, mixes them into the complete song, and exports Opus-encoded Ogg.

It statically links a trimmed FFmpeg build and ships its own minimal IFS/KBIN parser and Konami 4-bit ADPCM decoder, covering the whole pipeline in one pass — container unpacking, decoding, PCM mixing, mastering, Opus encoding, Ogg muxing. **No system FFmpeg needed.**

Three game branches are supported:

| Branch       | What has to be rebuilt                                               | Input                                         | Coverage                  |
|--------------|----------------------------------------------------------------------|-----------------------------------------------|---------------------------|
| **IIDX**     | the entire song is a sum of keysounds                                | chart `.1` + keysound archive `.s3p` / `.2dx` | 1st generation through 33 |
| **GITADORA** | drums / guitar / bass come from keysounds, laid over a pre-mixed bed | `m<id>_seq.ifs` + `m<id>_bgm.ifs`             | songs in FUZZ-UP          |
| **SDVX**     | nothing — the song ships as a finished file                          | `.s3v`, or a `.2dx` container                 | single-file transcode     |

This program contains no information whose copyright belongs to © Konami Arcade Games.

## Building

```
cargo build -r
```

`/vendor` already contains the prebuilt, trimmed FFmpeg static libraries. A first clean build requires the LLVM + VS 2022 MSVC toolchains; incremental builds do not.

## Usage

Commands are two levels deep — the game first, then the operation:

```
iidxOnKnitting iidx render  --ifs <.ifs>                       -o out.ogg  [-d spa]
iidxOnKnitting iidx render  --audio <.s3p|.2dx> --chart <.1>   -o out.ogg  [-d spa]
iidxOnKnitting iidx convert <file>                             -o out.ogg
iidxOnKnitting gd   render  --seq <_seq.ifs> --bgm <_bgm.ifs>  -o out.ogg
iidxOnKnitting sdvx convert <file>                             -o out.ogg
```

`render` rebuilds a song from its chart and keysounds; `convert` transcodes an already-mixed file as-is.

## Code layout

```
src/audio/      game-agnostic audio work (ADPCM decode / mixing timeline / mastering)
src/iidx/       chart.rs the chart · song.rs assembly
src/gitadora/   bmp.rs the bed · va3.rs keysounds · sq3.rs charts · song.rs assembly
src/unpack.rs   S3P0 (IIDX) and 2DX9 (shared by IIDX and SDVX) containers
src/codec.rs    libav FFI: decoding and Opus encoding
src/render.rs   mechanism: each entry does one thing, reporting what it cannot decide for you
src/run.rs      policy: run_iidx / run_gitadora / run_sdvx, one call per game
src/main.rs     argument parsing only; every leaf maps onto one run_*
```

### Using it as a library

There are two levels of entry point, and an embedder almost always wants the first:

```rust
use iidx_on_knitting::{run_iidx, run_gitadora, run_sdvx, IidxSource};

run_sdvx(&path_s3v, &path_out)?;                                   // .s3v or a .2dx container
run_gitadora(&path_seq_ifs, &path_bgm_ifs, &path_out)?;
run_iidx(IidxSource::Packed { ifs: &path_ifs, difficulty }, &path_out)?;
run_iidx(IidxSource::Loose { audio: &a, chart: &c, difficulty }, &path_out)?;
run_iidx(IidxSource::PreMixed(&path_pre_2dx), &path_out)?;         // transcode only
```

`run_*` returns a plain `anyhow::Result<()>` and **owns the two retry rules that have only one sensible answer**: a keysound archive that is really one pre-mixed file gets transcoded, and a file handed in as bare audio that is really a 2DX9 container goes through the container path. Both are decided from container magic before anything is decoded or written, so retrying costs nothing — this is policy the crate should own rather than have every consumer rewrite.

The second level is `render_*` / `convert_*`: each does exactly one thing and reports those two conditions as typed `RenderError` variants, for callers that want to branch themselves.

---

## Branch: IIDX

A song is a chart (`.1`) plus a keysound archive (`.s3p` or `.2dx`), either packed together in one `.ifs` or as loose files. Every note in the chart names a keysound, and the song is the sum of them — there is no backing bed at all. With no pre-mastered bed underneath, the sum's absolute level is arbitrary, so the output is only peak-normalised (scaled back proportionally, and only when the peak exceeds full scale).

`-d` picks which difficulty's event stream to read. Any difficulty rebuilds the same song, but not every song has every difficulty — `spn` is the only one guaranteed to exist.

> **Multi-source trade-off**: a few early songs pack multiple keysound `.2dx` archives in one folder / `.ifs` (~68 in this dataset — usually an old-vs-re-added pair, or per-difficulty sound sets). The renderer **auto-selects one** by the preference `<id>a` → `<id>1` → `<id>`; `<id>a` is usually the modern, re-added version (e.g. `01000a` for the 1st-gen song 01000). **This is an approximation** — the chosen archive is not guaranteed to be the "correct" arrangement for every difficulty, but it lets these songs render.

---

## Branch: GITADORA

A song is split across two `.ifs` archives, so both must be named:

- `m<id>_seq.ifs` — the charts (`d<id>.sq3` for drums, `g<id>.sq3` for guitar and bass) and the keysound archives (`spu<id>d.va3` / `spu<id>g.va3`; guitar and bass share the latter, separated only by sound_id range)
- `m<id>_bgm.ifs` — several pre-mixed backing tracks whose names, `bgm<id><d|_><g|_><b|_>k.bin`, encode **which parts are already mixed into that track**

The renderer picks the track with the fewest instruments baked in and reconstructs only the parts it lacks, so nothing is ever doubled. Most songs ship a backing-only `___k` bed, so drums, guitar and bass all come from keysounds; a few only ship `__bk`, in which case the bass stays as the game mixed it.

GITADORA difficulties do not affect the audio (the note sets of a part's difficulties are unioned and deduplicated), so there is no `-d` option here. It is natively 48 kHz, so the mix reaches Opus without any resampling.

Konami's 4-bit ADPCM is not part of FFmpeg; it is implemented in this project's own `src/audio/adpcm.rs` and needs nothing from `vendor/`.

> **Loudness trade-off**: GITADORA's bed is already a loudness-maximised master, so summing keysounds onto it pushes the median peak to about 2.2x full scale. The keysound layer is therefore attenuated by a fixed **0.70** and the remainder folded by a stateless soft knee (threshold 0.95). Measured across the library, the median render lands at −10.6 dBFS — level with the game's own pre-mixed masters — with only ~1% of samples touched by the knee. **The tail is the cost**: about 2.3% of songs have over 5% of their samples limited, and a few reach 17%, where the compression is audible. Removing that entirely would require per-song loudness normalisation instead.

> **Unsupported songs**: 1359 of the 1479 songs in the tested dataset render. For the remaining 120, both `.ifs` files are 1024-byte placeholders with a non-IFS magic — the song content simply is not in the dataset. In another 21 songs a few notes (0.4–2.2% of each) point at null sound ids like 0/1/2 and stay silent, the same way IIDX charts reference reserved sample numbers.

---

## Branch: SDVX

SDVX has no keysounds — songs ship as finished files, so there is nothing to reconstruct and only the container differs: `.s3v` is bare audio, `.2dx` is a 2DX9 container whose first entry is the mix. `convert` tells the two apart on its own.

---

## Third-Party Code Notice

The IFS-parsing code in `src/tool/ifs.rs` is based on the implementations of [![ifstools](https://img.shields.io/badge/ifstools-blue)](https://github.com/mon/ifstools) and [![kbinxml](https://img.shields.io/badge/kbinxml-blue)](https://github.com/mon/kbinxml).

The GITADORA field layouts in `src/audio/adpcm.rs` and `src/gitadora/` are based on [![gitadora-customs](https://img.shields.io/badge/gitadora--customs-blue)](https://github.com/fisyher/gitadora-customs), re-verified field by field against real data. Several of its assumptions did not hold for this dataset and are corrected here: `tick` counts 1/300 **second** units, not 1/300 ms; `volume` is a plain 0..127 linear value in every archive version, with no `VOLUME_TABLE` mapping; a VA3 entry's `filesize` can be trusted directly; and chunks flagged `is_metadata` can still carry real keysounds.

| Component                                                     | Version | Source                    | License           |
|---------------------------------------------------------------|---------|---------------------------|-------------------|
| FFmpeg (libavcodec / libavformat / libavutil / libswresample) | 8.0.2   | <https://ffmpeg.org/>     | LGPL-2.1-or-later |
| libopus                                                       | 1.5.2   | <https://opus-codec.org/> | BSD-3-Clause      |

Both static libraries are **trimmed builds** — only the components this tool needs are kept, and no GPL components are enabled:

| Category | Components kept                                         |
|----------|---------------------------------------------------------|
| Decoders | `wmav1` · `wmav2` · `wmapro` · `adpcm_ms` · `pcm_s16le` |
| Demuxers | `asf` (WMA inside s3p) · `wav` (RIFF/WAVE inside 2dx)   |
| Encoder  | `libopus`                                               |
| Muxer    | `opus` (Ogg-Opus)                                       |

Under each component's original license you may obtain its source, rebuild it yourself, and replace the prebuilt libraries in `vendor/`. This project's MIT license covers only the project's own source under `src/`; it does **not** cover the FFmpeg/libopus headers and prebuilt static libraries in `vendor/`.
