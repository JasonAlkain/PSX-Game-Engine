# Releasing

Editor builds for Windows are published to
[allyn-david.itch.io/copperframe-engine](https://allyn-david.itch.io/copperframe-engine)
by the GitHub Actions workflow `.github/workflows/release-itch.yml`, using
itch.io's uploader, butler. The full procedure, including the one-time
setup of the `BUTLER_API_KEY` secret, is in
[docs/RELEASING.md](https://github.com/JasonAlkain/PSX-Game-Engine/blob/main/docs/RELEASING.md).

## In short

1. One time: create the itch.io project page and add an itch.io API key as
   the repository secret `BUTLER_API_KEY`.
2. Tag a commit on `main` and push the tag:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

3. The workflow builds on `windows-latest` and pushes the bundle to the
   `windows` channel with the tag (without the `v`) as the version.

To test a build without publishing, run the workflow manually from
**Actions > Release to itch.io > Run workflow** with "publish" unticked and
download the bundle from the run's artifacts.

## What the bundle contains

- the editor, the `copperframe` tool and the stock player (release builds,
  with `.pdb` files so crash backtraces have line numbers);
- the example project's data (not its game crate, which needs the engine
  source to build);
- a launcher that opens the example project, and `.itch.toml` so the
  itch.io app's Launch button does the same;
- `README.txt`, `LICENSE.txt` and `NOTICES.txt`.

## Before tagging

- CI is green on `main`.
- `docs/STATUS.md` describes what this version has and has not verified.
- Format or API changes are in `docs/MIGRATION.md`.
- Ideally, the Windows checklist in
  [docs/PLATFORMS.md](https://github.com/JasonAlkain/PSX-Game-Engine/blob/main/docs/PLATFORMS.md)
  has been run on real hardware.

## This wiki

The wiki is built with mdBook and published to GitHub Pages by
`.github/workflows/docs.yml` on every push to `main` that changes
`docs/wiki/`. Pull requests build it as a check without publishing.
