# glabrs

Lean, fast CLI for self-hosted GitLab (17.0+). A focused Rust alternative to `glab`: one small binary, instant startup, no telemetry, no AI features — just the daily-driver command set with script-friendly JSON output everywhere.

## Install

```shell
cargo install glabrs
```

## Quickstart

```shell
# Authenticate (token is hidden-prompted, validated, stored 0600)
glabrs auth login --hostname gitlab.corp.example

# Works from any git checkout: remote URL is parsed to find host + project
glabrs mr list --assignee=@me
glabrs mr create --fill --draft --push
glabrs mr approve 235
glabrs issue create --title "Bug" --label important --milestone v2.1
glabrs ci status --wait          # exit code mirrors pipeline result
glabrs ci trace test-job         # live job log, streamed by polling
glabrs ci trace build --follow --pipeline 235   # job in a specific pipeline
glabrs release create v1.2.0 --notes-file -
glabrs api projects/42/issues --paginate --page 2 | jq '.[].title'
```

## Commands

| Group | Commands |
|---|---|
| `auth` | `login`, `status`, `logout` |
| `config` | `get`, `set`, `unset` |
| `api` | authenticated passthrough to any REST v4 endpoint (GET, POST, PUT, PATCH, DELETE) |
| `repo` | `view`, `list`, `clone` |
| `issue` | `list`, `create`, `view`, `close`, `reopen`, `comment`, `delete` |
| `mr` | `list`, `create`, `view`, `diff`, `approve`, `merge`, `close`, `reopen`, `comment`, `ready`, `update` |
| `ci` | `list`, `status`, `view`, `trace`, `run`, `retry`, `cancel`, `lint` |
| `release` | `list`, `create`, `view`, `delete`, `upload` |
| `completion` | `bash`, `zsh`, `fish`, `powershell` |

## Exit codes (for scripting)

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Runtime error |
| `2` | Usage error (clap) |
| `3` | Auth invalid / missing |
| `4` | Empty result / not found |

## Configuration

Files live in the platform config dir: `~/.config/glabrs/` on Linux,
`~/Library/Application Support/glabrs/` on macOS:

- `config.toml` — global settings (`api_url`, `pager`, custom `headers`)
- `hosts.toml` — per host: `token`, `api_url`, `ca_file`, `client_cert`/`client_key`, custom `headers`

Precedence: flags > env (`GLABRS_HOST`, `GITLAB_HOST`, `GITLAB_TOKEN`) > per-host > global.
Empty or whitespace-only env values count as unset and fall through to the
next source (the same rule as `GITLAB_TOKEN`, below).

### Token precedence

`--token` flag (= `GITLAB_TOKEN` env, applied by clap) > `CI_JOB_TOKEN` (when in CI) > stored token.
An empty or whitespace-only `GITLAB_TOKEN` counts as unset and falls through to
the next source. `auth login` honors the same head of the ladder: the
subcommand's `--token` beats `GITLAB_TOKEN`; `--token -` explicitly reads the
token from stdin.

Note: a token passed as `--token <value>` is visible to other local users via
the process list (`ps`). On shared machines prefer `GITLAB_TOKEN`
(`export GITLAB_TOKEN=...`) or `auth login` over the flag.

Inside CI, `CI_JOB_TOKEN` + `CI_API_V4_URL` are detected automatically — no stored
credentials needed. Job tokens are sent as `JOB-TOKEN` and authenticate
project-scoped endpoints only; features that need `GET /user` (for example
`mr list --assignee=@me`) require a personal access token and fail with exit
code 3 under a job token.

Note: OS trust roots are used by default, so corporate proxies with a private
root installed in the system store work without extra configuration.

Note: http-only instances (no TLS) need an explicit `api_url` in config —
derived URLs always use `https://`.

Note: credentials are stored with unix permissions (0600) and the binary
targets unix systems (Linux, macOS).

Note: MSRV. `rust-version` tracks the oldest toolchain the pinned
`Cargo.lock` actually builds (1.88 today; several locked crates ship
edition-2024 manifests and rustc >= 1.86 requirements). CI proves it.

## Self-hosted connection guide

### Self-signed certificates

Per-host CA bundle in `hosts.toml`:

```toml
["gitlab.corp.example"]
ca_file = "/etc/ssl/corp/root-ca.pem"
```

### mTLS client certificates

```toml
["gitlab.corp.example"]
client_cert = "/etc/ssl/corp/client.pem"
client_key  = "/etc/ssl/corp/client.key"
```

(mTLS fields are stored by `config set --hostname`; the TLS client itself
currently honors `ca_file`.)

### Custom ports and path-prefixed instances

Remote detection keeps the full path, so subgroups
(`group/subgroup/repo.git`) work out of the box. For path-prefixed reverse
proxies, set `api_url` explicitly:

```toml
["gitlab.corp.example"]
api_url = "https://gitlab.corp.example/gitlab/api/v4"
```

Hostnames are normalized (case-insensitive; `:443`/`:80` are treated as the
default ports and dropped; **non-default ports are part of the host identity
on every source** — `auth login --hostname`, `--hostname`, env, `CI_API_V4_URL`,
`-R` URLs, and git-remote detection all canonicalize to the same
`host:8443` spelling; scheme forms keep only the authority — any `user:token@`
credentials and URL path are stripped), so
`auth login --hostname GitLab.Corp.Example:443` and a checkout on
`https://gitlab.corp.example/...` resolve to the same entry, and a
`:8443` checkout matches the `:8443` entry `auth login` wrote. A custom header
whose name/value cannot be sent safely (CR/LF, `:` in the name, control
characters) is rejected as a usage error before any request.

### Escape hatch

`--insecure` skips TLS certificate verification (prints a loud warning; the
token travels over an unverified connection). Prefer `ca_file`.

## Scripting

- `--json` on list/view commands emits the raw GitLab API response — pipe straight to `jq`.
- `--paginate` follows `X-Next-Page` until exhausted (hard cap: 10,000 pages).
- `--paginate --page` starts fetching at the given page (every list command and
  `api --paginate`; sent exactly once, no duplicated parameter). List commands
  accept `--page` alone as a single-page fetch; `api` requires `--paginate`
  with `--page`. `--page` must be ≥ 1 and `--per-page` must be 1–100 — both are
  rejected by argument validation otherwise.
- HTTP 404 exits `4` (not found) and HTTP 401 exits `3` (auth), so scripts can
  distinguish a typo'd IID from a network failure. A `429` is retried once,
  honoring `Retry-After`.
- `ci status` / `ci trace` mirror the pipeline/job result: terminal failure
  exits `1`. Without `--wait` / `--follow` they are snapshots — a
  still-running status prints the current state and exits `0`.
- `--verbose` prints API method and path only — never headers or tokens.
- `--description-file -` reads stdin. `mr diff` (and `--web`-less long views)
  use `GLABRS_PAGER` > `PAGER` > config `pager` > `less`; `--no-pager`
  bypasses it. `mr create --fill` titles the MR from the newest commit's
  subject (with its body as the description).
- `-R` accepts a numeric ID, `owner/name`, `group/sub/name`, an
  already-encoded path, or a full URL with a path — a bare URL with no path
  (`https://gitlab.com`) is a usage error, and a URL pointing at a different
  host than the target instance is refused (pass `--hostname` explicitly).
- `issue delete` / `release delete` require `--yes` in non-interactive
  sessions (a piped stdin can never confirm); an interactive `[y/N]` refusal
  exits `2`, not `0`.
- `config get` reports unknown keys with exit `4`; `config unset` refuses a
  no-op on a missing key the same way (like `git config --unset`).

## License

MIT
