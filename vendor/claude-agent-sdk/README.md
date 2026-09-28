# vendor/claude-agent-sdk — vendored copy of `seher-claude-agent-sdk`

The root `Cargo.toml` applies `[patch.crates-io]` so workspace builds resolve
this checked-in copy of `seher-claude-agent-sdk = "=0.0.59"`. Cruise workspace
packages are not published to crates.io. This directory is intentionally
**not** a workspace member.

`scripts/verify_vendored_crate.sh` checks the dependency pin, patch resolution,
upstream archive checksum, and byte identity. It runs in CI (`PR Auto-merge` →
`lint`). The version pin and this copy can only move together, so Renovate is
disabled for this dependency (`renovate.json5`).

## Provenance

| | |
|---|---|
| Source | crates.io published `.crate` |
| URL | https://static.crates.io/crates/seher-claude-agent-sdk/seher-claude-agent-sdk-0.0.59.crate |
| Crate | `seher-claude-agent-sdk` 0.0.59 (lib name `claude_agent_sdk`) |
| `.crate` SHA-256 | `eb71dbe2c93dc3e12135e34d8236e676550b3f6ec3f1bb8473d0e465face6d82` |
| Verified against | sparse index `https://index.crates.io/se/he/seher-claude-agent-sdk` (`cksum`, `yanked=false`) |
| Upstream repo | https://github.com/smartcrabai/seher (`crates/claude-agent-sdk`) |
| Upstream commit | `6adf4624ed27084a0d16f1574e609c4eff84d7cc` (from `.cargo_vcs_info.json`) |
| License | Apache-2.0 |
| Copyright holder | takumi3488 (`smartcrabai`), from the packaged manifest's `authors` |

Upstream carries no `NOTICE` file and leaves the license appendix boilerplate
(`Copyright [yyyy] [name of copyright owner]`) unfilled, so the row above is the
attribution record for this redistributed copy.

Re-verify or refresh:

```sh
bash scripts/verify_vendored_crate.sh
```

## Modifications

None. Every file below `src/`, `examples/`, `Cargo.toml`, `Cargo.toml.orig`,
`Cargo.lock`, and `.cargo_vcs_info.json` is byte-identical to the published
`.crate` contents.

Added files, not present in the `.crate`:

- `LICENSE` — Apache-2.0 text copied verbatim from the upstream repository
  (`../seher/LICENSE`, SHA-256 `c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4`).
  The packaged manifest declares `license = "Apache-2.0"` but ships no license
  file; it is included here to satisfy Apache-2.0 redistribution.
- `README.md` — this provenance note. The packaged manifest sets
  `readme = false`, so it is not consumed by cargo.

Bugs are fixed in the cruise-side glue, not here. Any future change to this
directory must be a full re-extraction of a published `.crate` with this
README's provenance table updated.
