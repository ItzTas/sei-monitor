# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# sei-monitor

A monitor-hub plugin. It logs in to SEI through SEI's REST module (`mod-wssei`) with the owner's
account, lists the processes of the watched units every `POLL_INTERVAL`, and records what changed as
events in Postgres. The hub reads those events and sends the notifications. The plugin is written in
Rust on the monitor-hub plugin SDK (`monitor-hub-plugin`, git tag `sdk-rust/v0.1.1`).

The image does not run on its own. It holds `/plugin/plugin.json` and `/plugin/sei-monitor`, and the
hub's image build copies that directory in. Release `1.0.0` was a standalone Go service with its own
notifications, feed, API and web page. Its code is gone from the tree. The tag `v1.0.0` stays as the
reference for the golden vectors, the compatibility tests and the rollback.

## Code

- **Do not write comments in the code.** No `//`, `///` or `//!`. Names explain what the code does.
  Attributes such as `#[cfg(test)]` are not comments.
- **English everywhere**: identifiers, file names, SQL, logs, error messages, event text and docs.
- Only make `pub` what another module or the integration tests in `tests/` use. Everything else is
  `pub(crate)` or private.
- `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings` stay clean.
  Do not add `#[allow]` or `#[expect]` to get there.
- Prefer guard clauses and small functions over `else` branches, a map over a chain of
  `match`/`if` arms when the code only maps keys to values, and run independent calls together.
- The Go release `v1.0.0` is the specification. Log messages, error texts and command output keep
  Go's wording and fields. A difference from Go is a bug unless it is one of the accepted
  divergences listed in the README.
- All instants are UTC (`chrono::Utc`). The only time zone is `SEI_TIMEZONE`, from the table built
  into the binary (`chrono-tz`). Nothing reads `TZ` or the local zone.
- The SEI password, the session token and `DATABASE_URL` never appear in a log, an error, a gRPC
  message, an event, command output or a `Debug` output. They live in `Secret` (prints
  `[redacted]`, zeroed on drop), and `PluginConfig`, `SeiConfig`, `Credentials` and `Store` do not
  derive `Debug`.
- Code that calls SEI concurrently drops the pending calls at the first error and never spawns a
  detached task for a SEI call. Each wrong password costs one rejected login, and enough of them
  lock the account.

## Commands

```bash
cargo build
cargo test                    # see Tests for what needs Postgres, go or the conformance command
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
make check                    # fmt + clippy + test
make up                       # docker compose up -d: development Postgres on 127.0.0.1:5432
make down
docker buildx build --platform linux/amd64,linux/arm64 .
git config core.hooksPath .githooks   # once per clone
```

The pre-commit hook runs `cargo fmt --check` and `cargo test`. The commit-msg hook enforces
Conventional Commits. The hook and the `Makefile` run cargo with `rustup run <channel>` from
`rust-toolchain.toml` when `rustup` is installed. A cargo from the system package ignores that file,
and under another standard library `gotext::tests::case_mappings_match_go_for_every_char` fails.

## Tests

Some tests need tools from outside the crate. When one is missing they print a `skipping:` notice
on stderr and return. When `CI` is set and not empty they panic instead, so CI never passes by
skipping.

- Postgres: the SDK test kit's `database()` gives each test its own database. It uses
  `TEST_DATABASE_URL` when set (the compose Postgres works, see `.env.example`), and otherwise
  starts a throwaway cluster with `initdb` and `postgres` from `PATH`.
  `tests/support::migrated_database` adds the migrations. With `TEST_DATABASE_URL` every test binary
  shares one server with password authentication, as in CI. So a test that needs a role of its own
  creates it with `support::login_role` (create or alter, with a password) under a name no other
  test uses, and connects with that password.
- `go`, `git` and the tag `v1.0.0` for `tests/go_compat.rs`. It extracts the Go release with
  `git archive v1.0.0`, builds it and caches the binary under `target/tmp/go-v1.0.0/`. A shallow
  clone has no tag, so CI checks out the full history. These two tests take about 35 s, because
  they wait for Go's 30 s notification poll.
- The conformance command for `tests/conformance.rs`: the binary named by `PLUGIN_CONFORMANCE`, or
  else `go install` of `plugin-conformance` at the SDK's Go tag `sdk/v0.2.1`.

The tests that call the fake SEI fail when `HTTP_PROXY` is set in their environment.

The SDK refuses Unix socket paths longer than 100 bytes. The tests that start the plugin put its
socket in a short directory under the system temporary directory, so a long `TMPDIR` breaks them.

No test talks to a real SEI. `tests/support/fake_sei.rs` and its modules are a port of the Go
release's fake server.

CI: `.github/workflows/ci.yml` runs fmt and clippy in one job and `cargo test --locked` in another,
with a `postgres:18-alpine` service, Go 1.27.2 (`.prototools` pins the same version) and the
conformance command installed. `.github/workflows/docker.yml` builds the image for amd64 and arm64
and publishes it to `ghcr.io/itztas/sei-monitor` on every run that is not a pull request. A `v*`
tag publishes the version tags (`2.0.0` and `2.0`), only the default branch moves `latest`
(`flavor: latest=false` keeps the semver rules from moving it too), and every image gets a `sha-`
tag.

## Golden vectors

`testdata/golden/` holds files generated by the Go release `v1.0.0`: configuration, URLs,
durations, Go text rules, time zones, the SEI wire format and error classification, the login page,
event text, the diff, the monitor's texts, a few cycles and the operator commands. The tests read
them with `include_str!` and must match them. The generator left with the Go code, so the files are
frozen. A difference is a bug in the Rust code, never a reason to edit a vector.

`testdata/golden/html_entities.json` is also production data: `src/sei/html.rs` and
`src/sei/orgs/entity.rs` embed it at compile time, and the image build copies it in.

`src/gotext/testdata/simple_case.txt` lists Go's case mappings for every code point. Its small
generator, `simple_case.go` next to it, uses only Go's standard library and stays in the tree; the
test that reads the list prints the command that regenerates it.

`testdata/sei/` holds SEI answers recorded as files, used by the fake server and the vectors.

## Configuration

Under monitor-hub, set these on the hub with the `SEI__` prefix (`SEI__SEI_URL` and so on). The hub
removes the prefix before it starts the plugin. The plugin reads only its environment, never a
`.env`. `config::PluginConfig::load` collects every problem and reports them together, with Go's
texts, and never shows a secret value.

- `DATABASE_URL` (required, secret, or `DATABASE_URL_FILE`): `postgres://` or `postgresql://`. The
  plugin connects without TLS.
- `SEI_URL` (required): the SEI address without `/sei`. https, or http for localhost only.
- `SEI_API_URL`: the `mod-wssei` address, when discovery fails.
- `SEI_USERNAME` and `SEI_ORG` (required). `SEI_ORG` is the agency acronym or numeric id.
- `SEI_PASSWORD`, `SEI_PASSWORD_FILE` or `SEI_PASSWORD_CMD` (exactly one). The file is read at
  start-up. The command runs at every login, without a shell, with a 30 s limit and without the
  secret variables in its environment (also behind a `PREFIX__`, and `AMQP_URL`); start-up checks
  that its program is on `PATH`.
- `SEI_UNITS` (unit acronyms or `all`; empty is the user's current unit), `SEI_TIMEZONE`
  (`America/Sao_Paulo`), `SEI_CONCURRENCY` (3, from 1 to 10), `POLL_INTERVAL` (`15m`, at least
  `5m`), `RETURN_WARNING_DAYS` (2, from 0 to 60), `RETENTION_DAYS` (90, from 0 to 3650, at least
  `RETURN_WARNING_DAYS + 2` unless 0), `NOTIFICATION_DETAIL` (`minimal` or `full`), `LOG_LEVEL`
  (`info`).

The hub also passes `PATH`, `TZ`, `TMPDIR` and `MONITOR_HUB_DATA_DIR`. The plugin ignores `TZ` and
the data directory and keeps all its state in Postgres. Apart from the program of
`SEI_PASSWORD_CMD`, it runs no external program.

## Module map

```
src/
├── main.rs                 hardens the process, parses the arguments, builds the runtime, exit code
├── lib.rs                  module declarations only
├── harden.rs               RLIMIT_CORE 0, PR_SET_DUMPABLE 0, umask 077
├── app.rs                  plugin start-up: writer lock, migrations, store, monitor, serve, shutdown
├── server.rs               SeiPlugin: Describe, Status, Check and Events
├── cli/                    clap commands and exit codes (mod.rs), orgs and test-login
│                           (commands.rs), diagnose (diagnose.rs)
├── config/                 Source, ConfigError, SeiConfig, PluginConfig (mod.rs); the problem
│                           collector (reader.rs); SEI_URL and SEI_API_URL as Go's net/url parses
│                           them (url.rs, url/); the PATH lookup for SEI_PASSWORD_CMD (lookpath.rs)
├── gotext.rs               Go's text rules: quoting, EqualFold, Fields, case tables, Atoi, capped
├── duration.rs             Go's ParseDuration and Duration.String
├── golden.rs               test-only: reads the cases of a golden vector file
├── timezone.rs             zones, local dates to UTC with Go's time.Date rule, days_until
├── secret.rs               Secret
├── version.rs              VERSION, User-Agent
├── password.rs             the password sources and the password command
├── event/                  Kind and Payload (mod.rs), titles and bodies (format.rs), the payload
│                           the hub receives (view.rs)
├── sei/                    the SEI client: HTTP clients with the body limit (http.rs), login, session
│                           renewal and listings (client.rs), module discovery (discovery.rs), the
│                           login page and its agencies (orgs.rs, orgs/), connection (connect.rs),
│                           the JSON envelope and wire types (wire.rs, wire/, json.rs, types.rs),
│                           errors and their classification (errors.rs), HTML entities (html.rs)
├── store/                  Postgres: MIGRATIONS, the writer lock, the pool (mod.rs); accounts,
│                           cycle runs, processes and the cycle transaction, events and pruning
└── monitor/                Monitor and its state (mod.rs), the diff (diff.rs), activities and
                            documents (details.rs), pruning (prune.rs), the scheduler, pause and
                            back-off (scheduler.rs)
sql/schema/                 the goose migrations, embedded by store/mod.rs
tests/                      one file per area; tests/support/ holds the fake SEI, the fake clock,
                            the process helpers, the Go builder and a fake webhook for Go
```

## Process contract

- The hub starts `sei-monitor plugin` with its cookie variable. Started any other way, or with no
  command, the binary prints the SDK's `NOT_LAUNCHED_BY_HUB` text on stderr and exits 1.
- Stdout carries only the handshake line. Logs go to stderr as JSON lines.
- Start-up reads the configuration, takes the database writer lock (a Postgres advisory lock on its
  own connection), applies the migrations on that connection, opens the pool and reads the highest
  event position ever issued, all before the handshake. Each connection attempt has 10 s and the
  whole sequence has 45 s, because the hub kills a plugin that has not answered in 60 s. The plugin
  does not call SEI before the handshake.
- Exit code 0 is a clean stop (SIGTERM or SIGINT). When the plugin exits 1 because of a
  configuration error, a failed start-up step or a lost writer lock, it writes exactly one JSON
  `fatal` line at `error` level, whose `error` field holds the configuration problems, names the
  start-up step that failed, or starts with `the database lock was lost`. A run outside the hub and
  a failure to start logging or the runtime also exit 1, without a `fatal` line. The hub restarts
  the plugin with a back-off.
- After the handshake the process exits only on a signal, a lost writer lock or an error of the
  SDK's `serve` (a `fatal` line and exit 1). An unknown agency or module, a failing cycle, a
  rejected login and a panic in the scheduler leave it running, with the reason in `Status`,
  because an exit would make the hub restart it in a loop.
- A second process on the same database fails at start-up with `another sei-monitor process holds
  the database lock`, migrates nothing and does not call SEI.
- On SIGTERM the running cycle is cancelled at once and recorded as `context canceled`. The SDK
  drains the server for at most 1 s.
- `Describe` names the plugin `sei` (`SEI`) with eight kinds. `account_login_failed` and
  `return_overdue` are urgent, the rest are info.
- `Status`, first match wins: hub ahead (below), `FAILING`; an unknown agency or module while
  preparing, or a panicked scheduler, `FAILING` with the text; a cycle running, `CHECKING`; paused,
  `PAUSED` (below); the last cycle failed, `FAILING` with its error (the same text as
  `cycle_runs.error`); account not known yet, `FAILING` with `sei-monitor has not reached SEI
  yet[: <error>]`; the database does not answer within 10 s or the last `Events` read failed,
  `FAILING` with `the database is unavailable`; otherwise `IDLE`. Messages are cut at 500 characters. `last_success_at` is the last cycle run that finished
  without an error.
- `Check` starts a cycle, or refuses with Go's texts: not reached SEI yet, already running, less
  than 5 minutes since the last start, or paused.
- `Events(after, limit)` returns the events of the current account with an id above `after`, in
  order, exactly `min(limit, available)` of them, except during the first read. A `limit` of 0 or
  less means 100, and the maximum is 500. `seq` and `id` are `events.id`. Each call acknowledges
  `after` in memory (it never moves back and is not stored). Before the account is known `Events`
  answers `UNAVAILABLE`, and so does a database error (`the database is unavailable`).
- The first read stops at the highest position issued before start-up. Until one `Events` call
  returns a page shorter than its limit, newer events are left out. The hub stores its first read
  of a plugin as a baseline and announces none of it, so this keeps the first cycle's events out of
  the baseline. They go out on the next read. The limit belongs to one process: a restart before
  the hub's first read ended lets the new baseline take those events in.
- When `after` is above the highest position this database ever issued, the plugin's database was
  reset and the hub's position was not. `Events` then returns nothing and acknowledges nothing,
  logs `the hub is ahead of this database: reset the hub's position for the sei plugin` once, and
  `Status` shows that text until the two agree.
- A stored event that cannot be read (unknown kind or a payload that does not decode) is skipped
  and logged once at `error` as `skipping a stored event that cannot be read`, with its id and no
  stored value. After 1,024 such ids one summary line replaces the rest. One call walks at most
  100,000 rows, then answers `UNAVAILABLE`.

## Flow of a cycle

1. `Monitor::prepare` connects (module discovery, the agency from the login page, no login) and
   finds or creates the account, keyed by the normalized `SEI_URL`, `SEI_USERNAME` and the agency
   id. A failure is retried with back-off, capped at `POLL_INTERVAL`. An unknown agency or module
   sets `State::fatal` (logged once per distinct text) and is retried the same way, because one
   maintenance page from SEI at start-up reads as "module not found"; a preparation that works
   clears it. Preparation never logs in, so these retries cost the account nothing.
2. If the account holds a pause reason from an earlier run, the monitor is held (see Pause).
3. A cycle logs in when it has no session, selects the units, lists their processes (`SEI_CONCURRENCY`
   calls at a time) and diffs them with the stored ones. It then reads the activities and documents
   of the processes that need them, and writes everything in one transaction (`apply_cycle`):
   processes, documents, events in Go's order, deduplicated by `(account, kind, ref)`. The account
   row is locked first, so the order of ids is the order of the commits.
4. The first cycle of an account is a baseline: it records what is there and creates no events.
5. A failed cycle sets `State::cycle_error` to the error chain, stored also in `cycle_runs.error`;
   a successful one clears it. After a successful cycle pruning runs.
6. The scheduler waits `POLL_INTERVAL` plus up to a tenth of it, or 30 s doubling up to the
   interval after a failure. `Check` starts a cycle early.

The SEI client has a 30 s total timeout per request, reads at most 16 MiB of a body, stops at 400
process pages, refuses redirects on API calls, and cuts SEI text at 500 characters where it enters.
The NUL character is dropped from SEI text in the same place, since Postgres refuses it in `TEXT`.
TLS is verified against the root certificates built into the binary (`webpki-root-certs`).

## Pause

A login that SEI rejects pauses the monitor. The process stays up, records the reason on the
account and one `account_login_failed` event (ref `login:<time>`), and makes no other login. In
that process `Check` is refused. When the reason write fails, the parked scheduler writes it again
with the back-off until it lands or the process stops, without calling SEI.

A process that starts with a reason on record is held: it does not log in, logs `SEI rejected the
login in an earlier run; ...`, and records one `account_login_failed` event with ref
`login-held:<UTC date>`, so at most one per day however often it restarts. The hub restarts the
plugin on its own (after a lost lock, for example), so a restart is not the owner's act and must
not cost a login. A requested `Check` is the only retry. As soon as SEI accepts its login, the
reason is cleared in memory and in the database, so a later failure in that cycle follows the
ordinary back-off with the session it has. A clear that fails is tried again after the next
successful cycle. Only a check whose login got no verdict (5xx, a time-out, a page that is not the
API's) goes back to held.

## Pruning

With `RETENTION_DAYS` above 0, every successful cycle deletes, in its own transaction, the events
detected before `start - RETENTION_DAYS` (computed in UTC) and the processes that left before then.
Two locks guard the events:

1. Never delete an event with an id above the hub's acknowledged position. That position lives in
   memory and starts at 0, so nothing is pruned after a restart until the hub reads again.
2. An acknowledged event stays until the cutoff, so a `return_due_soon` is not deleted early and
   created again with a new id.

Deleting an event also deletes the Go release's `deliveries` rows for it (`ON DELETE CASCADE`).

## Persistence

- The schema is the Go release's, unchanged: `sql/schema/00001_initial.sql` and
  `00002_cycle_runs.sql`, applied by the SDK's `migrate::up` with goose's version table, so the Go
  release reads the same database.
- A schema change is a new file `sql/schema/0000N_name.sql` in goose format (`-- +goose Up` and
  `-- +goose Down`) and a new entry in `MIGRATIONS` in `src/store/mod.rs`. **Never edit a migration
  that already exists.** The Go release starts on a database that holds a migration it has no file
  for, so a new migration keeps the way back to Go open only while it leaves alone everything Go
  reads and writes: tables, columns, constraints and the event `ref` formats. A migration that
  changes one of those breaks the rollback, and nothing stops Go from starting on that database:
  the damage shows as wrong or failing queries. The same holds for a `ref` format changed in code:
  Go would announce the event again after a rollback.
- All SQL lives in `src/store/`. Never insert explicit ids: the columns are
  `GENERATED ALWAYS AS IDENTITY`.
- Pool connections run with a 20 s `statement_timeout`, a 5 s `lock_timeout`, TCP keepalives and a
  60 s `tcp_user_timeout`. The writer lock's connection has the same keepalives but no statement
  timeout, because it runs the migrations.
- The `deliveries` and `whatsmeow_*` tables belong to the Go release. The plugin never reads or
  writes them.

## Rollback to the Go release

The image `ghcr.io/itztas/sei-monitor:1.0.0` runs the Go release on the same database, with no SQL
step. The README lists what the switch costs in both directions.
