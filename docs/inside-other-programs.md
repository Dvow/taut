# taut inside other programs

Details: [chapter 9](09-inside-another-program.md).

taut need not own the window. With its own renderer (Direct3D 9–12, OpenGL, Vulkan, on Windows
and Linux), a program keeps a `Surface`, forwards input, and shows the surface as a texture:

```rust,ignore
ui.run(&mut describe);                             // when something happened
for [left, top, right, bottom] in surface.update(&mut ui) {
    // draws just the places that changed again: copy these pixels into your texture
}
```

- Pixels: B, G, R, A bytes, premultiplied (Direct3D `B8G8R8A8_UNORM`, blend "one, one minus
  source alpha").
- `ui.theme.bg = Color::NONE` makes the background transparent, for overlays.
- `platform::system_text()` uses system fonts on any surface; on Windows, `platform::Messages`
  turns another window's messages into events.
- Direct3D 11 and OpenGL 3.3+: `layer_d3d11::Layer::new(device, context)` or
  `layer_opengl::Layer::new(find)` once, then `layer.draw(&mut ui)` each frame after the
  program's drawing. It uploads only changed regions (`UpdateSubresource`, `glTexSubImage2D`),
  draws one quad at the viewport, restores all state, and returns errors as values. Unused,
  it isn't compiled in. Examples: `d3d11.rs`, `opengl.rs`; `vulkan.rs` by hand; wgpu in
  chapter 9. `tests/renderers.rs` compares each with the surface.
- No layers for Vulkan, Direct3D 9 or 12, or Metal.
- `examples/overlay.rs`: everything as a DLL with a C interface; `tests/dll.rs` loads and
  drives it.
