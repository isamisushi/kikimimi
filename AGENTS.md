# Desktop releases

Build and publish macOS Desktop releases locally from the root of this repository
on a Mac configured for signing and notarization. See [desktop/README.md](desktop/README.md)
for prerequisites and signing setup.

1. Check the version and changelog, run `npm run check --workspace desktop`, and
   commit and push the intended revision. Start from a clean checkout.
2. For a requested public release, run `npm run release:local --workspace desktop -- --publish`.
   The script detects the Mac, builds both CPU architectures, signs and notarizes
   both packages, and publishes the GitHub Release and updater manifest.
   No GitHub Actions workflow or architecture selection is required.
   To stage both packages locally, omit `--publish`.
3. After a notarization interruption, use `--resume --publish` on the same
   revision once you have checked that the versioned release is still unpublished.
4. Verify the versioned GitHub Release contains a DMG, updater archive and
   signature for each CPU. Check that the `desktop-updates` release's
   `latest.json` points both platforms at this version.
