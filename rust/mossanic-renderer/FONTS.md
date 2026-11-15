# Font rendering — deferred to v2

## Status
The Vulkan pipeline is complete and validated:
- skrifa parses the font successfully (measured 460.80px for "hello world!")
- zeno rasterizes individual glyphs correctly (39x86 for the cursor block)
- Vulkan swapchain, texture atlas upload, and pipeline creation all work

## What doesn't work
The layout layer. We tried in order:
1. **swash** — silent `None` returns from `Render::scale()`
2. **ab_glyph** — worked, but produced upside-down text (shader bug, fixable)
3. **suzuri 0.2** — silently drops 10 of 11 glyphs from the layout engine

## Plan for v2
Revisit text rendering when:
- suzuri reaches 1.0 or a stable successor exists
- OR ab_glyph coordinate issue is properly resolved
- OR custom layout on top of skrifa + zeno is written (skrifa measures correctly,
  zeno rasterizes correctly — only the layout loop was missing)

The 8x8 bitmap font approach (st, dvtm style) is a valid fallback if no library matures.

## Do not
Re-add swash, suzuri, or ab_glyph to Cargo.toml without reading this file first.
