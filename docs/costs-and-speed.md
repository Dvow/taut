# Costs and speed

Sizes, memory, CPU, and speed beside Dear ImGui, egui, libpng and libjpeg-turbo, each with its
date. Methods: [how it is tested](how-it-is-tested.md).

## What it costs

Sizes use this release profile (with Cargo's default, `hello` is 733 KB on Windows, 1,602 KB on
Linux):

```toml
[profile.release]
opt-level = "s"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

The demo (`examples/gallery.rs`; [Windows](windows.png), [Linux](linux.png)):

| | Windows 11 | Linux (Ubuntu 26.04, X11) |
| --- | --- | --- |
| Private memory | 2.5 MB (3.1 MB after using every page) | 3.5 MB after using every page |
| Working set, with shared libraries | 10.2 MB (11.5 MB after) | 12.2 MB after |
| Program size | 1,721 KB | 2,031 KB |
| CPU while idle | none | none |
| Dependencies | 0 | 0 |

- Memory: 2026-11-11, about 0.5 MB of it the demo's data. Sizes: 2026-11-15. Arch Linux: 4.0 MB
  private, 13.5 MB total.
- Accepting files dropped from other programs costs about 3.5 MB on Windows, so a window only
  accepts them once its app asks.

A program contains only the parts it calls: icons, shadows, pictures, clipboard pictures and
files, variable-font axes, drag out, SVG extras, PDF, printing, the tray and the system file
dialog are compiled in only when called. What calls can't reveal is a feature, off by default:

| Feature | Adds | Without it | Windows (hello; gallery) | Linux (hello; gallery) |
| --- | --- | --- | --- | --- |
| `right-to-left` | Hebrew, Arabic, Persian, Urdu in Unicode order; Arabic joined | letters left to right, as typed | 206 KB; 135 KB | 193 KB; 144 KB |
| `brahmic` | Indic and South-East Asian scripts shaped by syllable | system text: whole on Windows, letter by letter on Linux | 280 KB; 209 KB | 277 KB; 228 KB |
| `emoji` | colour emoji from the system font | plain letters: text colour on Windows; outlines if any on Linux | 142 KB; 55 KB | 140 KB; 71 KB |
| `fallback-fonts` | Windows: missing letters in the font Windows would pick | Windows' linked fonts (Linux always uses Fontconfig) | 38 KB; 28 KB | nothing |
| `touch` | touch, pen and pinch gestures | a finger is the system's mouse (Wayland: the first finger) | 30 KB; 12 KB | 40 KB; 19 KB |
| `cff` | CFF and CFF2 fonts (most `.otf`) | not read | nothing; 17 KB | nothing; 17 KB |
| `color-profiles` | colour profiles in picture files | colours as stored | nothing; 7 KB | nothing; 7 KB |
| `woff2` | WOFF2 fonts (with a Brotli decoder) | not read | nothing; 95 KB | nothing; 99 KB |

- Each cost is the feature alone, in `hello` and in the gallery (which reads fonts and
  pictures). `brahmic` adds 77 KB (Linux 86 KB) to `hello` with `right-to-left` already on.
- `hello`: 457 KB on Windows, 758 KB on Linux; `bench`: 492 KB and 582 KB (2026-11-15).
- Linux is larger by Rust's backtrace support (about 165 KB) and by carrying X11, Wayland and
  AT-SPI, since which is needed is known only at run time.

Same window, same profile (Windows, 2026-11-15): egui 0.37 (eframe, wgpu) 8.3 MB, 179 crates;
iced 0.14 (wgpu, tiny-skia) 5.9 MB, 159 crates; Slint 1.18 (femtovg, software) 7.8 MB, 245
crates. Private memory (2026-11-11): egui 126.7 MB, iced 17.2 MB, Slint 7.3 MB.

## How fast it is

`cargo run -p taut --example bench --release`: a bar and rows of seven boxes, no screen. The same
window in Dear ImGui (Rust bindings) and egui, same profile and machine, in turns, 2026-11-12;
best of 20 runs (egui: 8). µs:

| Rows | taut: describe | taut: redraw changes | taut: both | Dear ImGui: one frame | egui 0.37: one frame |
| --- | --- | --- | --- | --- | --- |
| 100 | 19 | 12 | 31 | 38 | 656 |
| 1,000 | 160 | 12 | 172 | 244 | 6,114 |
| 10,000 | 1,578 | 12 | 1,590 | 2,316 | 67,263 |

Dear ImGui and egui frames end at GPU triangle lists; taut's include finished pixels.

| Rows | Pointer moves, nothing new under it | Button press to pixels |
| --- | --- | --- |
| 100 | 12 ns | 34 µs |
| 1,000 | 12 ns | 246 µs |
| 10,000 | 12 ns | 2,380 µs |

- Describing and redrawing beats one Dear ImGui frame at every size; 20–40× faster than egui.
- Dear ImGui usually runs every frame; egui redraws everything on every mouse move; taut redraws
  only what changed.
- Redrawing changes costs the same at any row count. A full redraw (1200×800) takes about
  280 µs.
- A press is the mean of press and release; release describes twice (click, then rerun).
- Benchmarks use the built-in font; system fonts measure each new string once.
- Long lists belong in `rows`, `list` or a table, so cost doesn't grow with length.
- Dragging describes at most twice per move; moving a panel doesn't re-lay out.

### Picture files

PNG reading and writing are the library's own. Against libpng 1.6.57 and zlib 1.3.1 on Ubuntu,
same machine, 2026-11-12, in turns, best of 15, identical pixels; a 1920×2160 photo and a
2700×1800 screenshot; ms:

| Reading | libpng | taut |
| --- | --- | --- |
| Photo, RGB | 48.2 | 36.8 |
| Photo, RGBA | 54.0 | 40.5 |
| Photo, interlaced | 62.3 | 39.5 |
| Photo, 16-bit | 130.1 | 124.0 |
| Photo, 256 colours | 15.7 | 6.4 |
| Screen, RGB | 40.6 | 19.6 |
| Screen, RGBA | 45.6 | 22.0 |
| Screen, 256 colours | 18.0 | 6.1 |

| Writing (ms, size) | libpng default | libpng fastest tried | taut |
| --- | --- | --- | --- |
| Photo | 670, 3.10 MB | 110, 4.05 MB | 87, 3.50 MB |
| Screen | 135, 314 KB | 29, 358 KB | 21, 346 KB |

libpng's default is about 10% smaller and 6–8× slower. (Fastest tried: lowest level; Sub filter
for the photo, None for the screen.) Arch Linux, libpng 1.6.58: reading 1.03–2.9× as fast.

JPEG writing, a 3840×2160 photo, Arch Linux, against libjpeg-turbo 3.2.0 (byte-identical
output), 2026-11-14, sequential; ms:

| Quality, size | libjpeg-turbo SIMD | libjpeg-turbo C | taut |
| --- | --- | --- | --- |
| 75, 438,687 bytes | 27.9 | 142.8 | 101.5 |
| 95, 1,521,291 bytes | 51.7 | 245.9 | 188.9 |

No SIMD (it would need `unsafe`).

## Why it is small and fast

- Runs record boxes in one flat list; layout is a fixed number of passes; only changed regions
  redraw (a blinking cursor: a 3 px strip).
- Everything layout reads is hashed while describing; an unchanged hash skips layout. Constant
  arguments hash at compile time.
- Unchanged text isn't re-measured; drawing visits only visible boxes; no recursion.
- A strip buffer (about 256 KB up to 4096 px wide), not a back buffer. (Wayland: the client owns
  the window's pixels and draws into them.)
- System text: GDI on Windows, FreeType and Fontconfig on Linux.
- No frame loop: events and requested wake-ups only.
- System functions declared in the library; Wayland spoken directly; Linux libraries loaded at
  run time. Nothing linked, no headers.
