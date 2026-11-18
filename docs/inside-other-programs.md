# taut inside other programs

Details: [chapter 9](09-inside-another-program.md).

taut need not own the window. With its own renderer (Direct3D 9–12, OpenGL, Vulkan, on Windows
and Linux), a program keeps a `Surface`, forwards input, and shows the surface as a texture:

```rust,ignore
ui.run(&mut describe);                             // when something happened
for place in surface.update(&mut ui) {
    // draws just the places that changed again: copy these pixels into your texture
    let up = surface.upload(place);                // x, y, width, height, bytes, bytes_per_row
}
```

- Pixels: B, G, R, A bytes, premultiplied (Direct3D `B8G8R8A8_UNORM`, blend "one, one minus
  source alpha").
- `ui.theme.bg = Color::NONE` makes the background transparent, for overlays.
- `platform::system_text()` uses system fonts on any surface; on Windows, `platform::Messages`
  turns another window's messages into events.
- Layers for Direct3D 9, 11 and 12, OpenGL 3.3+, Vulkan and Metal: made once from the program's
  device (`layer_d3d9`, `layer_d3d11`, `layer_d3d12`, `layer_opengl`, `layer_vulkan`,
  `layer_metal`), then
  `layer.draw(..)` each frame after the program's drawing. Each uploads only changed regions,
  draws one quad, and returns errors as values; Direct3D 9 and 11 and OpenGL restore all state
  they set. Unused, a layer isn't compiled in. Examples: `d3d9.rs`, `d3d11.rs`, `d3d12.rs`,
  `opengl.rs`, `vulkan.rs`; wgpu in chapter 9. `tests/renderers.rs` compares each with the
  surface.
- Any other renderer: `surface.upload(place)` gives each changed place as a texture upload
  takes it.
- `examples/overlay.rs`: everything as a DLL with a C interface; `tests/dll.rs` loads and
  drives it.
