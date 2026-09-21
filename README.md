# hkmu-ole-attendance

Personal use for taking attendance for Hong Kong Metropolitan University's
Online Learning Environment (OLE).

> Rewritten in Rust: pure HTTPS, no browser, no WebDriver.

This will NOT work for lectures or tutorials that are using iBC's iAttend, as it
requires a Bluetooth signal which this program cannot produce. For courses that
use the teacher's **class activities** attendance, this works.

## How it works

The program authenticates against OLE, asks the `oledb` API for the day's
timetable, and then — once each class has started — submits attendance on the
class-activities page, polling every 10 minutes until the submission is
confirmed or the class ends.

Everything is done with ordinary HTTPS requests. There is no headless browser,
no geckodriver, and no Xvfb; the container is a single statically-linked binary
on a slim base image.

## Authentication

Two methods are supported. **`SESSION_COOKIE` takes precedence when set.**

### 1. Session cookie (recommended)

Log into OLE in a browser, open DevTools → Network, pick any request to
`iole.hkmu.edu.hk`, and copy the whole `Cookie` request header. Paste it as
`SESSION_COOKIE`.

This skips the login chain entirely and avoids storing your password.

**Important:** an `LtpaToken` is valid for exactly 4 hours, and its expiry is
written when it is issued — polling does not extend it. Since a pasted cookie
cannot be renewed by the program itself, `SESSION_COOKIE` suits short manual
runs. For unattended operation use the password method below, which logs in
again and receives a fresh token whenever the cached one ages out.

```
SESSION_COOKIE=LtpaToken=...; IPCZQX03a626c736=...; ZNPCQ003-38383600=...
```

### 2. Student ID and password

If `SESSION_COOKIE` is unset, set both `STUDENT_ID` and `STUDENT_PASSWORD` and
the program will drive the full SSO chain itself (NAM login → SAML assertion →
Domino silent sign-on → dashboard). This survives cookie expiry unattended, at
the cost of storing your password.

### How the session is kept alive

The authenticated session is held in memory and reused for as long as its
`LtpaToken` remains valid, refreshing only within five minutes of expiry. A
poll therefore costs one request rather than a six-hop login, and the client
does not re-authenticate on every check. Re-logins are logged as
`cached session is at end of life; re-authenticating`.

## Deployment

### Docker

The image is ~5.7 MB. It is built `FROM scratch` and contains a single fully
static musl binary — no shell, no libc, no package manager, and no system CA or
timezone store. TLS roots (`webpki-roots`) and the IANA timezone database (the
bundled `jiff` tzdb) are compiled into the binary, so there is nothing left to
install or keep patched at runtime.

```bash
# From the published registry (amd64 and arm64)
docker run --rm --env-file .env ghcr.io/chaosoffire-private/hkmu-ole-attendance:latest

# Or build locally
docker build -t hkmu-ole-attendance .
docker run --rm --env-file .env hkmu-ole-attendance
```

No inbound ports are required.

### Direct

```bash
cargo build --release
./target/release/hkmu-ole-attendance
```

## Continuous integration and releases

`.github/workflows/ci.yml` runs on every push and pull request:

| Job          | Checks                                                                             |
| ------------ | ---------------------------------------------------------------------------------- |
| `rustfmt`    | `cargo +nightly fmt --check` (needs nightly for import grouping)                   |
| `clippy`     | `cargo clippy --all-targets --all-features -- -D warnings`                         |
| `test`       | `cargo test --all-features --locked`                                               |
| `cargo-deny` | advisories, bans, licenses, sources                                                |
| `image`      | builds the image, runs `--version`/`--help` inside it, and asserts no shell exists |

`.github/workflows/release.yml` publishes to GHCR on a `v*` tag (or manually via
`workflow_dispatch`). It is gated on the full CI workflow, builds
`linux/amd64` and `linux/arm64` with provenance and SBOM attestations, and tags:

- `latest` on every `v*` tag
- `{{version}}`, `{{major}}.{{minor}}`, `{{major}}` from a semver tag
- the short commit SHA
- the branch name, when dispatched manually against a branch

```bash
git tag v0.3.0 && git push origin v0.3.0
```

## Configuration

| Variable           | Default                    | Meaning                                                                |
| ------------------ | -------------------------- | ---------------------------------------------------------------------- |
| `SESSION_COOKIE`   | —                          | Raw `Cookie:` header value. Takes precedence over credentials.         |
| `STUDENT_ID`       | —                          | Student ID used for the SSO login.                                     |
| `STUDENT_PASSWORD` | —                          | Password used for the SSO login.                                       |
| `DISCORD_WEBHOOK`  | —                          | Discord webhook URL. When unset, notifications are written to the log. |
| `SCHEDULE_TIME`    | `03:00`                    | Time of the daily setup, in `HH:MM`.                                   |
| `TIMEZONE`         | `Asia/Hong_Kong`           | Timezone for all scheduling and class times.                           |
| `OLE_URL`          | `https://iole.hkmu.edu.hk` | OLE portal base URL.                                                   |
| `RUST_LOG`         | `info`                     | Log filter, e.g. `debug` for verbose output.                           |

`SCHEDULE_TIME` accepts `H:MM` or `HH:MM`. Trailing `#` comments are stripped,
so `docker --env-file` inline comments are harmless. An invalid value is
rejected at startup rather than silently defaulting.

## Command line

```bash
# Run the daily scheduler (default)
hkmu-ole-attendance

# Authenticate, print today's classes, exit
hkmu-ole-attendance --fetch-only

# Send today's classes to the Discord webhook, exit
hkmu-ole-attendance --notify-only

# Report whether each class's attendance activity is reachable
hkmu-ole-attendance --probe

# Submit specific coordinates with attendance (default 0,0)
hkmu-ole-attendance --coordinates 22.3364,114.1796
```

`--fetch-only` and `--probe` are useful for checking that a refreshed
`SESSION_COOKIE` still works.

## Reporting and notifications

Every message goes through one reporting component, so output is structured
`tracing` output in all cases — there is no separate print path. Each call
declares whether it should also be mirrored to Discord:

| Call | Behaviour |
| --- | --- |
| `Mirror::Log` | Logged only. Used for `--fetch-only` and `--probe` diagnostics. |
| `Mirror::Discord` | Logged, **then** posted to Discord when a webhook is set. |

Discord delivery is awaited inside the same call, so a mirrored report is
complete once it returns. A delivery failure is logged and swallowed: reports
are advisory, and a webhook outage must never prevent attendance from being
recorded.

Create a webhook via `Edit Channel > Integrations > Webhooks > New Webhook` and
put the URL in `DISCORD_WEBHOOK`. You will be notified when the class list is
retrieved, when attendance is confirmed, once when 30 minutes remain without a
confirmation, and when a class ends without a confirmed submission. With no
webhook set, every mirrored report is still logged, so the container logs remain
the complete record.

Log detail is controlled by `RUST_LOG` (e.g. `debug` shows session reuse and
successful deliveries). Session tokens and the webhook URL are redacted in all
log output.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

The crate denies `unwrap`, `expect`, `panic`, `todo`, `unimplemented`, and
slice indexing, so those constructs cannot reach production code.
