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
glabrs release create v1.2.0 --notes-file -
glabrs api projects/42/issues --paginate | jq '.[].title'
```

## Commands

| Group | Commands |
|---|---|
| `auth` | `login`, `status`, `logout` |
| `config` | `get`, `set`, `unset` |
| `api` | authenticated passthrough to any REST v4 endpoint |
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

Files live in `~/.config/glabrs/`:

- `config.toml` — global settings (`api_url`, custom `headers`)
- `hosts.toml` — per host: `token`, `api_url`, `ca_file`, `client_cert`/`client_key`, custom `headers`

Precedence: flags > env (`GLABRS_HOST`, `GITLAB_HOST`, `GITLAB_TOKEN`) > per-host > global.

### Token precedence

`--token` flag > `GITLAB_TOKEN` env > `CI_JOB_TOKEN` (when in CI) > stored token.

Inside CI, `CI_JOB_TOKEN` + `CI_API_V4_URL` are detected automatically — no stored
credentials needed. Job tokens are sent as `JOB-TOKEN` and authenticate
project-scoped endpoints only; features that need `GET /user` (for example
`mr list --assignee=@me`) require a personal access token and fail with exit
code 3 under a job token.

Note: OS trust roots are used by default, so corporate proxies with a private
root installed in the system store work without extra configuration.

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
default ports), so `auth login --hostname GitLab.Corp.Example:443` and a
checkout on `https://gitlab.corp.example/...` resolve to the same entry.

### Escape hatch

`--insecure` skips TLS certificate verification (prints a loud warning; the
token travels over an unverified connection). Prefer `ca_file`.

## Scripting

- `--json` on list/view commands emits the raw GitLab API response — pipe straight to `jq`.
- `--paginate` follows `X-Next-Page` until exhausted (hard cap: 10,000 pages).
- `--verbose` prints API method and path only — never headers or tokens.
- `$EDITOR`-free workflows: `--description-file -` reads stdin; `PAGER` (or
  `GLABRS_PAGER`) is honored for long output; `--no-pager` bypasses it.

## License

MIT
