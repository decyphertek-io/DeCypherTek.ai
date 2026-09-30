# DeCypherTek.ai — Workflows, Branches & Releases

How CI/CD works in this repo: which workflow does what, what the branches
mean, and how two contributors work side by side without stepping on each
other. **The branches are the contributors** — each developer gets their
own `dev-<name>` branch and their own prerelease channel.

## The Workflows

| Workflow | Trigger | Branch it runs on | What it produces |
| --- | --- | --- | --- |
| `dev-adminotaur.yml` | push + manual | `dev-adminotaur` | rolling prerelease `v<version>-dev-adminotaur` |
| `dev-usaginotsuki.yml` | push + manual | `dev-usaginotsuki` | rolling prerelease `v<version>-dev-usaginotsuki` |
| `Prod-Build.yml` | manual | `main` | stable release `v<version>` (`releases/latest`), re-published fresh every dispatch |
| `sonarqube.yml` | manual (`workflow_dispatch`) | whichever branch you dispatch it on | security findings committed to `appsec/` |

All dev workflows run the full gate before shipping anything: rustfmt
(`--check`), clippy (`-D warnings`), `cargo test`, then the same
cross-compile matrix as Prod-Build (Linux aarch64/armv7/x86_64 musl +
macOS aarch64/x86_64), a smoke test, checksums, packaging.

## The Branches (the contributors)

- `main` — **production**. Only Prod-Build publishes from here; the
  install script serves whatever `releases/latest` points at. Never build
  experiments directly on `main`.
- `dev-adminotaur` — **adminotaur's channel**. Every push runs tests and
  overwrites the channel's single rolling prerelease
  `v<version>-dev-adminotaur` in place — the branch name is in the tag,
  so it can never be confused with (or overwrite) the other channel.
- `dev-usaginotsuki` — **usaginotsuki's channel**. Same pipeline, same
  gates, rolling prerelease `v<version>-dev-usaginotsuki`.

A new contributor follows the same pattern: copy
`dev-adminotaur.yml` to `dev-<yourname>.yml`, change the name, branch,
concurrency group, artifact prefix and tag suffix, push branch and
workflow together — CI does the rest. The release sweep matches your
suffix, so your channel only ever overwrites its own releases — never
another contributor's, never the stable one.

## The Releases (two channels)

- **Stable** — output of Prod-Build on `main`. Served by the install
  script (`releases/latest`), doubles as the updater.
- **Prereleases** — output of the dev pipelines. Marked *pre-release*, so
  GitHub excludes them from `releases/latest`. They can never leak onto
  a user's device; you have to grab one on purpose.

### Working side by side

Each contributor pushes to their own `dev-*` branch, so neither can break
the other's pipeline. To test each other's work:

1. Open **Actions → dev-<name> → latest successful run → the release**
   (or the Releases page, filter for the prerelease tag).
2. Download the binary for your device and run it next to (or instead of)
   your own build. The vault, config and memory live under
   `~/.decyphertek.ai/` regardless of which binary you run — keep
   separate test homes by using a scratch `$HOME` if you want isolation.
3. Comment findings on the branch's commits/PR; fixes land on that
   `dev-*` branch and flow to `main` only via an explicit promotion
   (merge + manual Prod-Build), after both sides agree.

Each dev channel keeps exactly one prerelease: every push overwrites
it in place (assets `--clobber` + refreshed notes) and sweeps away the
channel's own older releases (previous versions' rolling tags and the
legacy `v<version>-<branch>.<sha>` pile), so the releases page never
grows past one stable + one prerelease per dev channel. A sweep matches
only its own tag names (`-dev-adminotaur`, `-dev-usaginotsuki`), so the
two dev channels can never overwrite each other's releases — only their
own. Prod-Build re-publishes its single stable release fresh on every
dispatch (new date, new notes, same one release) and never touches
prereleases.

## SonarQube (the security loop)

Run **Actions → SonarQube App Scan → Run workflow** on the branch you are
currently fixing (usually your own `dev-*` branch). The scan:

1. Pushes the branch's source to SonarCloud (`SONAR_TOKEN` secret, same
   org token the other repos in decyphertek-io use).
2. Pulls every open issue + security hotspot for **that branch**.
3. Commits `appsec/sonarqube-issues.json` (machine-readable) and
   `appsec/sonarqube-findings.txt` (readable) back to the same branch,
   `[skip ci]` so it never triggers builds.

Then read `appsec/`, fix findings on that branch, push, and re-run the
scan — the findings file should shrink. See `appsec/README.md`.
