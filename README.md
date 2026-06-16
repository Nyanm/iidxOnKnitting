# IIDX on Knitting

## 简介

IIDX on Knitting是一款BMS风格的，将谱面key音渲染在歌曲的背景音频文件中，最终导出为完整的Opus编码歌曲的程序。
程序实现了修剪版的FFmpeg静态链接集成和一个极简版的IFS/KBIN解析器，一站式实现从IFS信息提取，WMA/WAV/MS-ADPCM解码，PCM混音，OPUS编码与OGG封装的全流程。
程序覆盖初代到32代全部歌曲文件的转换功能。程序同时实现了SDVX中对于.s3v和.2dx音频文件的格式转换。

本程序不包含任何所属©Konami Arcade Games版权所有的信息。

## 使用

使用`cargo build -r`进行编译。`/vendor`内存放编译好的裁剪版FFmpeg二进制lib，无需再次编译，无需系统FFmpeg。
程序初次clean构建依赖LLVM + VS 2022 MSVC 工具链，增量构建不需要。

> **多音源曲的取舍**：极少数早期曲在一个文件夹 / `.ifs` 内含多个键音 `.2dx`（本库实测约 68 首，多为
> 新旧两版，或按难度分的音源）。此时程序按 `<id>a` → `<id>1` → `<id>` 的优先级**自动选其一**渲染——
> `<id>a` 通常是现代复活 / 重做版（如初代曲 01000 的 `01000a`）。**这是个近似取舍**：所选未必对每个难度都是
> "原配"音源，但能保证这些曲也出声。

## 第三方代码声明

`src/tool/ifs.rs`中关于解析IFS文件的代码参考了[![ifstools](https://img.shields.io/badge/ifstools-blue)](https://github.com/mon/ifstools)和[![kbinxml](https://img.shields.io/badge/kbinxml-blue)](https://github.com/mon/kbinxml)的实现。

| 组件                                                            | 版本    | 来源                        | 许可证               |
|---------------------------------------------------------------|-------|---------------------------|-------------------|
| FFmpeg (libavcodec / libavformat / libavutil / libswresample) | 8.0.2 | <https://ffmpeg.org/>     | LGPL-2.1-or-later |
| libopus                                                       | 1.5.2 | <https://opus-codec.org/> | BSD-3-Clause      |

以上静态库均为 **裁剪构建**，仅保留本工具所需的组件、未开启任何 GPL 组件：

| 类别       | 保留组件                                                  |
|----------|-------------------------------------------------------|
| 解码器 Decoder  | `wmav1` · `wmav2` · `wmapro` · `adpcm_ms` · `pcm_s16le` |
| 解复用器 Demuxer | `asf`（s3p 内 WMA） · `wav`（2dx 内 RIFF/WAVE）              |
| 编码器 Encoder  | `libopus`                                              |
| 封装器 Muxer    | `opus`（Ogg-Opus）                                       |

用户可依据各组件原许可证获取源码、自行重编并替换 `vendor/` 中的预编译库。本项目的 MIT 许可证仅覆盖 `src/` 下的本项目源码，**不覆盖** `vendor/` 中的 FFmpeg/libopus 头文件与预编译静态库。

---

# IIDX on Knitting · English

## Introduction

IIDX on Knitting is a BMS-style renderer: it lays a chart's keysounds onto the song's background-audio bed and exports the result as a complete, Opus-encoded track.
It statically links a trimmed FFmpeg build together with a minimal IFS/KBIN parser, covering the entire pipeline in one pass — IFS extraction, WMA/WAV/MS-ADPCM decoding, PCM mixing, Opus encoding, and Ogg muxing.
It converts song files from the very first generation through IIDX 32. It also achieves the format transmission of .s3v file and .2dx file in SDVX.

This program contains no information whose copyright belongs to © Konami Arcade Games.

## Usage

Build it with `cargo build -r`. The `/vendor` directory ships the prebuilt, trimmed FFmpeg static libs, so there is no need to recompile FFmpeg or to install a system FFmpeg.
A first clean build requires the LLVM + VS 2022 MSVC toolchains; incremental builds do not.

> **Multi-source trade-off**: a few early songs pack multiple keysound `.2dx` archives in one
> folder / `.ifs` (~68 in this dataset — usually an old-vs-re-added pair, or per-difficulty sound
> sets). The renderer **auto-selects one** by the preference `<id>a` → `<id>1` → `<id>`; `<id>a`
> is usually the modern, re-added version (e.g. `01000a` for the 1st-gen song 01000). **This is an
> approximation** — the chosen archive is not guaranteed to be the "correct" arrangement for every
> difficulty, but it lets these songs render.

## Third-Party Code Notice

The IFS-parsing code in `src/tool/ifs.rs` is based on the implementations of [![ifstools](https://img.shields.io/badge/ifstools-blue)](https://github.com/mon/ifstools) and [![kbinxml](https://img.shields.io/badge/kbinxml-blue)](https://github.com/mon/kbinxml).

| Component                                                     | Version | Source                    | License           |
|---------------------------------------------------------------|---------|---------------------------|-------------------|
| FFmpeg (libavcodec / libavformat / libavutil / libswresample) | 8.0.2   | <https://ffmpeg.org/>     | LGPL-2.1-or-later |
| libopus                                                       | 1.5.2   | <https://opus-codec.org/> | BSD-3-Clause      |

Both static libraries are **trimmed builds** — only the components this tool needs are kept, and no GPL components are enabled:

| Category | Components kept                                         |
|----------|--------------------------------------------------------|
| Decoders | `wmav1` · `wmav2` · `wmapro` · `adpcm_ms` · `pcm_s16le` |
| Demuxers | `asf` (WMA inside s3p) · `wav` (RIFF/WAVE inside 2dx)   |
| Encoder  | `libopus`                                               |
| Muxer    | `opus` (Ogg-Opus)                                       |

Under each component's original license you may obtain its source, rebuild it yourself, and replace the prebuilt libraries in `vendor/`. This project's MIT license covers only the project's own source under `src/`; it does **not** cover the FFmpeg/libopus headers and prebuilt static libraries in `vendor/`.
