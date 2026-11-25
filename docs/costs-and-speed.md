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
- Spell checking loads the system's checker the first time a checked box is typed in. On Ubuntu
  (Enchant with Hunspell's en_US) that is about 10.7 MB more, 7.4 MB of it the program's own
  (measured 2026-11-17 on the demo, which checks its text boxes since 0.2.1). Apps that never
  call `ui.check_spelling` load nothing of it.

A program contains only the parts it calls: icons, shadows, pictures, clipboard pictures and
files, variable-font axes, drag out, SVG extras, PDF, printing, the tray and the system file
dialog are compiled in only when called. What calls can't reveal is a feature, off by default:

| Feature | Adds | Without it | Windows (hello; gallery) | Linux (hello; gallery) |
| --- | --- | --- | --- | --- |
| `right-to-left` | Hebrew, Arabic, Persian, Urdu in Unicode order; Arabic joined | letters left to right, as typed | 206 KB; 135 KB | 193 KB; 144 KB |
| `brahmic` | Indic and South-East Asian scripts shaped by syllable | system text: whole on Windows, letter by letter on Linux | 280 KB; 209 KB | 277 KB; 228 KB |
| `shaping` | app fonts shaped as pages are; the system's text in the language's forms | glyph by glyph, pairs kerned; default forms | 178 KB; 154 KB | 202 KB; 166 KB |
| `emoji` | colour emoji from the system font | plain letters: text colour on Windows; outlines if any on Linux | 142 KB; 55 KB | 140 KB; 71 KB |
| `fallback-fonts` | Windows: missing letters in the font Windows would pick | Windows' linked fonts (Linux always uses Fontconfig) | 38 KB; 28 KB | nothing |
| `touch` | touch, pen and pinch gestures | a finger is the system's mouse (Wayland: the first finger) | 30 KB; 12 KB | 40 KB; 19 KB |
| `cff` | CFF and CFF2 fonts (most `.otf`) | not read | nothing; 17 KB | nothing; 17 KB |
| `color-profiles` | colour profiles in picture files | colours as stored | nothing; 7 KB | nothing; 7 KB |
| `rare-jpeg` | hierarchical and lossless arithmetic-coded JPEGs | not read | nothing; 10 KB | nothing; 12 KB |
| `woff2` | WOFF2 fonts (with a Brotli decoder) | not read | nothing; 95 KB | nothing; 99 KB |
| `hinting` | TrueType fonts' own hinting instructions run | every font autohinted | nothing; 93 KB | nothing; 96 KB |
| `gpu` | the GPU shows windows ([below](#gpu-or-cpu)) | the CPU shows them | 6 KB; 6 KB | 63 KB; 60 KB |
| `gpu-draw` | the GPU draws them too ([below](#drawn-by-the-gpu)) | the CPU draws them | 124 KB; 143 KB more than `gpu` | 82 KB; 104 KB more than `gpu` |
| `hdr` | boxes and pictures brighter than SDR white on HDR screens; Ultra HDR JPEGs | shown at SDR white | 48 KB; 65 KB more than `gpu` | 23 KB; 39 KB more than `gpu` |
| `translations` | built-in words in German, Spanish, French, Japanese, Portuguese, Chinese | English until set | 24 KB; 22 KB | 37 KB; 36 KB |
| `video` | video and sound by the system's player (`Video`) | none | nothing; nothing | nothing; nothing |

- `video` costs only where a video is opened: the `player` example is 88 KB more than `hello`
  on Windows, 52 KB on Linux (2026-11-24).
- On phones (arm64, 2026-11-25): `gpu` adds 17 KB to `hello` and the gallery on iOS, 34 KB on
  Android; `gpu-draw` 33 KB and 49 KB more on iOS, 76 KB and 85 KB more on Android.
- Each cost is the feature alone, in `hello` and in the gallery (which reads fonts and
  pictures). `brahmic` adds 77 KB (Linux 86 KB) to `hello` with `right-to-left` already on.
- `hinting`: a fresh page of text at 9–13 px takes Segoe UI 5.7 ms (3.6 autohinted), Arial
  24 ms (3.5): Arial runs about 3,000 instructions a glyph.
- `hello`: 457 KB on Windows, 758 KB on Linux; `bench`: 492 KB and 582 KB (2026-11-15).
- Linux is larger by Rust's backtrace support (about 165 KB) and by carrying X11, Wayland and
  AT-SPI, since which is needed is known only at run time.

Same window, same profile (Windows, 2026-11-15): egui 0.37 (eframe, wgpu) 8.3 MB, 179 crates;
iced 0.14 (wgpu, tiny-skia) 5.9 MB, 159 crates; Slint 1.18 (femtovg, software) 7.8 MB, 245
crates. Private memory (2026-11-11): egui 126.7 MB, iced 17.2 MB, Slint 7.3 MB.

## GPU or CPU

The CPU draws every pixel on every path; with `gpu` the GPU shows them. Windows 11, RTX 2080 Ti,
2026-11-16, while a game ran on the GPU; each case a program of its own on a private desktop,
200 frames, best of 2 (memory: range). CPU per frame, µs:

| | CPU (GDI) | Direct3D 11 | Direct3D 12 | Vulkan | OpenGL |
| --- | --- | --- | --- | --- | --- |
| 1200×800, all changed | 929 | 884 | 1,028 | 1,191 | 1,282 |
| 1200×800, one label changed | 273 | 507 | 731 | 898 | 756 |
| 3840×2160, all changed | 4,002 | 3,535 | 3,406 | 3,428 | 5,506 |
| 3840×2160, one label changed | 343 | 598 | 811 | 1,029 | 881 |
| First frame after start | 82–95 ms | 82–100 ms | 87–101 ms | 84–102 ms | 100–222 ms |
| GPU shows it from | | 0.4–0.5 s | 0.5–0.6 s | 0.8–0.9 s | 0.3–1.4 s |
| Private memory, 1200×800 | 2.1 MB | 58–62 MB | 104–105 MB | 142 MB | 84–96 MB |
| Private memory, 3840×2160 | 2.1 MB | 95–114 MB | 158 MB | 198 MB | 218–277 MB |

- A frame's CPU time includes running the app and its 10 ms timer, the same on every path.
- Each present costs CPU in the driver, whatever changed: about 230 µs (Direct3D 11), 460 µs
  (12), 630 µs (Vulkan, half on driver threads), 480 µs (OpenGL).
- First frames are the CPU's on every path. OpenGL's come later: its driver loads meanwhile, the
  likely cause.
- OpenGL uploads over 256 KB through a pixel buffer: straight from the program's memory, NVIDIA's
  driver took 15 ms a 4K frame.
- The memory is the GPU driver's and the window-sized textures.
- Gains: whole frames in step with the screen (the CPU path's strips can show half-drawn), no
  frame of nothing at start. Not a gain here: CPU time.

Linux (Ubuntu in WSL, Xvfb, gallery, 1200×800; app CPU + X server CPU per frame):

| | CPU path | GPU, llvmpipe | GPU, d3d12 (RTX 2080 Ti) |
| --- | --- | --- | --- |
| All changed | 1.6–1.8 + 1.3–1.7 ms | 11.0–11.3 + 3.0–3.3 ms | 2.6–2.7 + 3.0 ms |
| One small change | 0.03–0.04 + ~0 ms | 1.7–2.0 + 3.1–4.5 ms | 1.5–1.6 + 2.7–3.5 ms |
| Start | 96–290 ms | 169–193 ms | 432–474 ms |
| Memory | +9 MB | +108 MB | +150 MB |

- Xvfb has no DRI3, so Mesa sends every whole frame through X: that is why `Auto` keeps the CPU
  there. A real desktop with DRI3 was not measured.

Linux under Wayland (GitHub's Ubuntu runner, Weston 14's GL renderer on llvmpipe, dmabufs from
lavapipe: a software device, so its copies are CPU work too; 800×500, release build, medians of
60 frames, three runs):

| | Shared memory | Vulkan dmabufs |
| --- | --- | --- |
| One label changed: drawing | 5–10 µs | 33–92 µs |
| All changed: drawing | 26–80 µs | 0.11–0.77 ms |
| Shown (the compositor's frame callback) after | 16–25 ms | 16–25 ms |

- Drawing: painting, and for dmabufs the copy into an image and its wait. Shown: Weston's repaint
  cycle (16 ms on Xvfb, 25 ms headless), the same on both paths.
- On a software device dmabufs only add work: `Auto` keeps shared memory. A hardware GPU was not
  measured.

macOS (GitHub's runner: macOS 26.7, a VM with Apple's paravirtual GPU, one screen at scale 1;
2026-11-24; the cards page of [Drawn by the GPU](#drawn-by-the-gpu); each case a program of its own, 100 frames, best of 2). CPU
per frame, µs (show: Metal only shows; draw: it draws too, `TAUT_DRAW=gpu`):

| | CPU (Core Animation) | Metal, show | Metal, draw |
| --- | --- | --- | --- |
| 400×300, all changed | 721 | 1,231 | 1,189 |
| 400×300, one label changed | 685 | 1,124 | 1,382 |
| 400×300, list scrolled a row | 883 | 2,164 | 1,104 |
| 400×300, gradients and shapes | 1,254 | 3,054 | 1,584 |
| 1024×645 (all the screen), all changed | 3,575 | 3,134 | 1,279 |
| 1024×645, one label changed | 3,299 | 1,349 | 1,664 |
| 1024×645, list scrolled a row | 4,448 | 4,754 | 1,535 |
| 1024×645, a box moving | 3,410 | 1,553 | 1,631 |
| 1024×645, gradients and shapes | 7,780 | 7,834 | 1,576 |
| Most memory (whole process), 1024×645 | 97 MB | 100–108 MB | 95 MB |
| GPU shows it from | | 0.15–0.27 s | 0.16–0.59 s |

- The CPU path hands Core Animation a new picture of the whole window every frame (a copy of
  all its pixels); Metal uploads only the changed places: at 1024×645 a small change costs a
  third to a half.
- Each Metal frame costs about 0.5 ms more in the driver (a VM's here), so small windows cost
  more on Metal, as on Windows.
- Drawing on the GPU takes a third to a fifth of the CPU's time where much changes.
- Metal's frames wait for the screen (16.7 ms at 60 Hz); the CPU path's are handed over as fast as
  they come (0.7–9 ms), more than the screen shows.
- First frames are the CPU's: Metal is opened (its shaders compiled with `gpu-draw`) on a thread
  of its own.

## Drawn by the GPU

With `gpu-draw` the GPU draws too. Windows 11, RTX 2080 Ti, 2026-11-17, the PC busy with other
builds; a page of rounded, bordered cards with two labels each; each case a program of its own
on a private desktop, 100 frames, best of 2. CPU per frame, µs (show: the GPU only shows; draw:
it draws too):

| | CPU (GDI) | Direct3D 11, show | Direct3D 11, draw | OpenGL, show | OpenGL, draw |
| --- | --- | --- | --- | --- | --- |
| 400×300, all changed | 576 | 838 | 635 | 1,003 | 820 |
| 400×300, one label changed | 287 | 582 | 614 | 640 | 765 |
| 1920×1080, all changed | 12,286 | 12,048 | 5,561 | 13,995 | 5,854 |
| 1920×1080, list scrolled a row | 4,751 | 4,614 | 847 | 8,152 | 968 |
| 1920×1080, a box moving | 549 | 831 | 788 | 984 | 1,002 |
| 3840×2160, all changed | 66,844 | 67,917 | 47,897 | 65,807 (see below) | 50,382 |
| 3840×2160, list scrolled a row | 11,991 | 12,411 | 947 | 26,826 | 1,119 |
| 3840×2160, a box moving | 1,324 | 1,575 | 1,458 | 1,748 | 1,643 |
| 3840×2160, one label changed | 1,198 | 1,477 | 1,418 | 1,709 | 1,569 |
| Frame, 1920×1080 all changed | 33 ms | 33 ms | 17 ms | 28 ms | 21 ms |
| Private memory, 1920×1080 | 2 MB | 100 MB | 101 MB | 105 MB | 198 MB |

Direct3D 12 and Vulkan, the same page, 50 frames, one run, later that evening (the PC less
busy: compare within this table):

| | CPU (GDI) | Direct3D 11, draw | Direct3D 12, show | Direct3D 12, draw | Vulkan, show | Vulkan, draw |
| --- | --- | --- | --- | --- | --- | --- |
| 400×300, all changed | 459 | 545 | 894 | 812 | 1,003 | 1,005 |
| 400×300, one label changed | 256 | 488 | 641 | 810 | 892 | 990 |
| 1920×1080, all changed | 8,383 | 3,867 | 8,567 | 4,157 | 8,345 | 4,432 |
| 1920×1080, list scrolled a row | 3,519 | 643 | 3,187 | 868 | 3,265 | 1,136 |
| 3840×2160, all changed | 46,064 | 34,533 | 44,951 | 32,911 | 44,921 | 33,669 |
| 3840×2160, list scrolled a row | 9,809 | 865 | 9,815 | 1,247 | 9,502 | 1,339 |
| 3840×2160, one label changed | 937 | 1,144 | 1,465 | 1,372 | 1,612 | 1,522 |
| Private memory, 1920×1080 | 2 MB | 104 MB | 150 MB | 155 MB | 193 MB | 195 MB |

Gradients, filled shapes and strokes (cards of a gradient turning each frame, a filled wave and
its outline), 30 frames, best of 2: before, the CPU drew them into patches; now the GPU's own
math:

| | CPU (GDI) | Direct3D 11, patches | Direct3D 11 | Direct3D 12 | Vulkan | OpenGL |
| --- | --- | --- | --- | --- | --- | --- |
| 400×300 | 1,026 | 1,236 | 551 | 882 | 1,050 | 1,200 |
| 1920×1080 | 21,082 | 21,691 | 1,566 | 2,332 | 1,896 | 2,847 |
| 3840×2160 | 99,161 | 149,094 | 4,504 | 7,306 | 4,357 | 5,927 |
| Frame, 3840×2160 | 110 ms | 159 ms | 16 ms | 20 ms | 16 ms | 18 ms |

- Where the CPU fills or blends many pixels (large windows, scrolling), drawing on the GPU takes
  a fifth to a twelfth of the CPU time; with gradients and shapes, 13 to 22 times less.
  For small changes it costs what showing costs: a present.
- `Auto` therefore draws parts of 65,536 pixels or more (256×256) on the GPU, smaller ones on the
  CPU: below that both cost about a present.
- 4K, all changed: the cards' colours change every frame, so each label's run is drawn by GDI
  again on its new colour; that is most of the 33–48 ms.
- Direct3D 12 and Vulkan wait for each part they draw before the next; Direct3D 11 does not.
- A shape is drawn in bands of 16 rows, each pixel visiting the edges of its band; one with
  more than 256 edges in a band stays on the CPU. Blur (SVG filters only) stays on the CPU: a
  drawing is drawn once and kept, so the GPU would gain nothing a frame.
- First frames are the CPU's, as when showing. Direct3D 11 and 12 compile their HLSL on the
  device's thread; Vulkan's SPIR-V is shipped (made there on Windows, at the first drawn frame
  under X11); OpenGL compiles at the first drawn frame.
- Memory: two 2048×2048 textures (glyphs and runs kept; this frame's patches, stops and edges),
  on the GPU.
- Within 3 levels of 255 of the CPU in tests (WARP, SwiftShader, llvmpipe, lavapipe, the RTX):
  rounding of blends at edges and see-through fills. The gallery's walk: within 4 levels under
  X11 (OpenGL, Vulkan); on Windows within 13 (Direct3D 11 and 12, Vulkan), on GDI's text off one
  solid colour.
- The first measurements of OpenGL showing at 4K timed out: the PC was so busy that the device
  took up to 16 s and a frame up to 0.37 s, past the tool's 40 s. Run again, 4K takes 66 ms a
  frame; the tool now measures at most 20 s once the device came.

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
