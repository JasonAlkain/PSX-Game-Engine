# Releasing the editor to itch.io

The workflow `.github/workflows/release-itch.yml` builds the Windows editor
bundle and uploads it to
[allyn-david.itch.io/copperframe-engine](https://allyn-david.itch.io/copperframe-engine)
with [butler](https://itch.io/docs/butler/), itch.io's command-line uploader.

## One-time setup

1. Create the project page on itch.io (Dashboard > Create new project) with
   the URL `copperframe-engine`, kind "Downloadable". It can stay a draft.
2. Get an itch.io API key: go to
   [itch.io/user/settings/api-keys](https://itch.io/user/settings/api-keys)
   and click **Generate new API key**. Copy it.
3. Add it to GitHub: in this repository open **Settings > Secrets and
   variables > Actions > New repository secret**, name it `BUTLER_API_KEY`,
   and paste the key.

The key can upload to every itch.io project on the account, so keep it only
in the secret. If it leaks, revoke it on the same itch.io page and add a new
one.

## Publishing a version

Tag a commit on `main` and push the tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The workflow builds the release binaries on `windows-latest`, assembles the
bundle, and pushes it to the `windows` channel
(`allyn-david/copperframe-engine:windows`) with the tag (without the `v`) as the
version shown on itch.io. butler uploads only what changed since the last
push, and the itch.io app updates installed copies.

## Trying a build without publishing

Open **Actions > Release to itch.io > Run workflow**. Leave "publish"
unticked to only build; the bundle is attached to the run as an artifact to
download and test. Tick it to also push that build (versioned
`<crate version>-dev.<commit>`).

## What the bundle contains

- the editor, the command-line tool and the stock player (release builds)
- `examples/room` project data (not its Rust game crate, which needs the
  engine source to build)
- `Open example project.bat`, and a `.itch.toml` so the itch.io app's Launch
  button offers the editor (project hub) or the example project
- `README.txt`, `LICENSE.txt` and `NOTICES.txt` (third-party crate licences,
  generated from `cargo metadata`; the editor copies it into exports of
  projects without game code)

Executable and package names are set once in the workflow's `env:` block.
