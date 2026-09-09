# kikimimi brand assets

`kikimimi-mark.svg` is the editable source, based on the supplied logo reference.
The black and white marks have transparent backgrounds. `kikimimi-icon.svg`
uses a black rounded square with a white ear for app icons and favicons.
Page headers use the transparent black mark on light backgrounds and the white
mark on dark backgrounds (or invert the black mark with CSS).

After editing the source, run `node docs/scripts/generate-brand.mjs` from the
repository root (install `docs` dependencies first). This updates both color
variants, web and desktop copies, the favicon, and the desktop menu bar RGBA
asset. The desktop build generates PNG, ICNS, and ICO icons from `desktop/icon.svg`.

Open `preview.html` to compare the transparent marks at different sizes.

Transparent marks use a tighter viewBox so their strokes remain legible at header
and menu bar sizes. App tiles retain their original padding and declare a 2048 px
size for raster icon generation. The 36 px Retina menu bar image is rendered at
288 px first, then downsampled with Lanczos filtering for antialiased edges.
