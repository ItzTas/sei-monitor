# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# sei-monitor

A monitor for monitor-hub. It logs in to SEI through SEI's REST module (`mod-wssei`) with the owner's
account, lists the processes of the watched units every `POLL_INTERVAL`, and records what changed as
events in Postgres. The hub reads those events and sends the notifications. The monitor is written in
Rust on the monitor-hub SDK (`monitor-hub-sdk`, git tag `sdk-rust/v0.4.0`).

The image does not run on its own. It holds `/monitor/monitor.json` and `/monitor/sei-monitor`, and the
hub's image build copies that directory in. Release `1.0.0` was a standalone Go service with its own
notifications, feed, API and web page. Its code is gone from the tree.

## Code

- **Comments are short and only where they explain something worth explaining**: a guarantee, a
  rule that comes from outside the code, a pointer to this file. Write none where the code is
  obvious, and never one that repeats the code. Attributes such as `#[cfg(test)]` are not comments.
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
  `[redacted]`, zeroed on drop), and `Config`, `SeiConfig`, `Credentials` and `Store` do not
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

## Dependencies

- `sea-orm` and `sea-orm-migration` are pinned with `=` at 2.0.4, with default features off.
  sqlx gets no TLS feature: `cargo tree -e features -i sqlx-core | grep -c _tls` stays 0.
- After any change to `Cargo.toml`, set these back with `cargo update -p <crate> --precise
  <version>`: `sqlx` 0.9.0, `nix` 0.31.3, `uuid` 1.29.0, `winnow` 1.0.4, `toml_edit`
  0.25.17+spec-1.1.0, `toml_datetime` 1.1.2+spec-1.1.0, `regex` 1.13.1, `regex-automata` 0.4.18
  and `regex-syntax` 0.8.11. Check on crates.io that each new lock entry is at least two weeks
  old, and move a newer one back with `--precise`.
- `libsqlite3-sys` is in `Cargo.lock` and never compiled, and so are `sqlx-sqlite`, `sqlx-mysql`
  and the `arrow-*` crates. The sqlx features that sea-orm's defaults turn on carry weak
  `sqlx-sqlite?/...` edges, and cargo keeps the packages those edges name in the lock
  (cargo#10801). `cargo tree -i libsqlite3-sys --target all -e all` prints nothing. An audit that
  reads the lock still reports RUSTSEC-2026-0459 against `libsqlite3-sys` 0.37.0; look again when
  sqlx 0.9.1 is two weeks old, after 2026-12-02.

## Tests

Some tests need tools from outside the crate. When one is missing they print a `skipping:` notice
on stderr and return. When `CI` is set and not empty they panic instead, so CI never passes by
skipping.

- Postgres: `tests/support/postgres.rs`'s `database()` gives each test its own database. It uses
  `TEST_DATABASE_URL` when set (the compose Postgres works, see `.env.example`), and otherwise
  starts a throwaway cluster with `initdb` and `postgres` from `PATH`, listening on a local TCP
  port. `TEST_DATABASE_URL` must carry a password, like `DATABASE_URL`. The throwaway cluster
  trusts every login, so its URL carries one that Postgres ignores. Unit tests reach the same file
  as `crate::postgres`, through a `#[path]` in `src/lib.rs`.
- `tests/support::migrated_database` takes the writer lock and runs the migrations, as start-up
  does. With `TEST_DATABASE_URL` every test binary shares one server with password authentication,
  as in CI. So a test that needs a role of its own creates it with `support::login_role` (create
  or alter, with a password) under a name no other test uses, and connects with that password.
- `tests/migration.rs` runs the migrations on goose-era databases that `tests/support/schema.rs`
  builds (goose versions 1 and 2, a gap, an empty version table, the seventeen `whatsmeow_*`
  tables) and compares the catalog and the rows before and after.
- `tests/support/proxy.rs` is a TCP proxy that stops forwarding when told to. Two lock tests in
  `tests/store.rs` use it and wait out the whole 30 s loss limit, so that binary takes about 33 s.
- The conformance command for `tests/conformance.rs`: the binary named by `MONITOR_CONFORMANCE`, or
  else `go install` of `monitor-conformance` at the SDK's Go tag `sdk-go/v0.6.0`, which also checks
  `Watch` on the real binary.

The signal tests subscribe with `Notifier::subscribe` and wait for a signal with a bounded
`timeout` on `Signals::changed` (`support::next_signal`, 5 s), or look for one without waiting
(`support::pending_signal`). The process tests read the monitor's gRPC `Watch` stream, each message
within 5 s. None of them sleeps to wait for the monitor.

The tests that call the fake SEI fail when `HTTP_PROXY` is set in their environment.

The SDK refuses Unix socket paths longer than 100 bytes. The tests that start the monitor put its
socket in a short directory under the system temporary directory, so a long `TMPDIR` breaks them.

No test talks to a real SEI. `tests/support/fake_sei.rs` and its modules are a port of the Go
release's fake server.

CI: `.github/workflows/ci.yml` runs fmt and clippy in one job and `cargo test --locked` in another,
with a `postgres:18-alpine` service, Go 1.27.2 (`.prototools` pins the same version) and the
conformance command installed. `.github/workflows/docker.yml` builds the image for amd64 and arm64
and publishes it to `ghcr.io/itztas/sei-monitor` on every run that is not a pull request. A `v*`
tag publishes the version tags (`2.2.0` and `2.2`), only the default branch moves `latest`
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
removes the prefix before it starts the monitor. The monitor reads only its environment, never a
`.env`. `config::Config::load` collects every problem and reports them together, with Go's
texts, and never shows a secret value.

- `DATABASE_URL` (required, secret, or `DATABASE_URL_FILE`): the Postgres address, as
  `postgres://user:password@host:port/database?sslmode=disable` (`postgresql://` works too). Put
  the host, port, user, password and database in the URL. The configuration refuses any other
  scheme, the key=value form included, with Go's text `DATABASE_URL: use
  postgres://user:password@host/database`. Start-up checks the rest when it takes the writer lock.
  - The password goes in the address or in a `password` query key. A URL without one, or with an
    empty one, is refused with `DATABASE_URL has no password: add the role's password to the URL`.
    Without a password in the URL, sqlx would read `PGPASSWORD`, `PGPASSFILE` or `~/.pgpass`, and
    it logs a malformed `.pgpass` line whole at `warn`. A line missing a field, such as
    `*:*:*:secret`, puts the password where sqlx expects the user name, so the log would show it.
    With the refusal in place, the monitor never uses `PGPASSWORD` and never reads a `.pgpass` file.
    A database that trusts the role still needs a password in the URL, and Postgres ignores it.
  - Whatever else the URL leaves out, sqlx takes from the `PG*` variables (`PGPORT`, `PGUSER`,
    `PGDATABASE`, `PGOPTIONS`, `PGSSLMODE` and the rest), then the user from the system account.
    Under the hub the `PG*` variables only arrive when set as `SEI__PG*`.
  - The monitor connects without TLS. `sslmode=disable`, `allow` and `prefer` connect in plain
    text; `require`, `verify-ca` and `verify-full` fail at once.
  - These are refused with `DATABASE_URL is not a valid Postgres address`, which never repeats the
    address: several hosts (`h1,h2`), a URL with no host (unless a non-empty `host` or `hostaddr`
    query key names one), and any query key the monitor does not know. sqlx would log an unknown
    key together with its value, and that value can be a secret (`sslpassword`, a misspelled
    `Password`). When a URL has neither a host nor a password, the host is the one reported.
  - The monitor passes on the query keys sqlx reads: `sslmode`, `options` and `options[<setting>]`,
    `application_name`, `host`, `hostaddr`, `port`, `dbname`, `user`, `password`, the certificate
    keys (`sslrootcert`, `sslcert`, `sslkey`, their dashed forms and `ssl-ca`) and
    `statement-cache-capacity`. Key names are case sensitive.
  - It accepts and ignores the libpq keys that 2.1 took and sqlx has no equivalent for:
    `connect_timeout`, `tcp_user_timeout`, `keepalives`, `keepalives_idle`, `keepalives_interval`,
    `keepalives_retries` and `load_balance_hosts`, plus `channel_binding=disable` or `prefer`,
    `sslnegotiation=postgres` and `target_session_attrs=any`. Any other value of those last three
    stops the start with a message that names the key and never its value: `channel_binding` and
    `sslnegotiation` need TLS, and `target_session_attrs` needs a session check the monitor does not
    run.
  - An IPv6 host goes in brackets: `postgres://user:password@[::1]:5432/database`.
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

The hub also passes `PATH`, `TZ`, `TMPDIR` and `MONITOR_HUB_DATA_DIR`. The monitor ignores `TZ` and
the data directory and keeps all its state in Postgres. Apart from the program of
`SEI_PASSWORD_CMD`, it runs no external program.

## Module map

```
src/
├── main.rs                 hardens the process, parses the arguments, builds the runtime, exit code
├── lib.rs                  module declarations only
├── harden.rs               RLIMIT_CORE 0, PR_SET_DUMPABLE 0, umask 077
├── app.rs                  monitor start-up: writer lock, migrations, store, monitor, serve, shutdown
├── server.rs               SeiMonitor: Describe, Status, Check and Events
├── cli/                    clap commands and exit codes (mod.rs), orgs and test-login
│                           (commands.rs), diagnose (diagnose.rs)
├── config/                 Source, ConfigError, SeiConfig, Config (mod.rs); the problem
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
├── store/                  Postgres: Store, StoreError and ServerError (mod.rs); DATABASE_URL
│                           checks, connection options and the pool (connection.rs); the writer
│                           lock, its 0.5 s check and migrate (lock.rs); the SeaORM entities
│                           (entity/); accounts (accounts.rs), cycle runs (runs.rs), processes and
│                           the cycle transaction (cycle.rs), events and pruning (events.rs), the
│                           records the store returns (records.rs)
├── migration/              Migrator, the migrations in order (mod.rs); catch_up, the goose files
│                           a database has not recorded (goose.rs); the two migrations
│                           (m20261118_000001_goose_schema.rs, m20261118_000002_drop_go_tables.rs)
└── monitor/                Monitor and its state (mod.rs), the diff (diff.rs), activities and
                            documents (details.rs), pruning (prune.rs), the scheduler, pause and
                            back-off (scheduler.rs)
sql/schema/                 the goose files, read by the first migration; never edited
tests/                      one file per area; tests/support/ holds the fake SEI, the fake clock,
                            the process helpers, the throwaway Postgres, goose-era databases and a
                            TCP proxy
```

## Process contract

- The hub starts `sei-monitor monitor` with its cookie variable. Started any other way, or with no
  command, the binary prints the SDK's `NOT_LAUNCHED_BY_HUB` text on stderr and exits 1.
- Stdout carries only the handshake line. Logs go to stderr as JSON lines.
- Start-up reads the configuration, takes the database writer lock (a Postgres advisory lock on a
  connection of its own), applies the migrations with `sea-orm-migration` on that connection, opens
  the pool and reads the highest event position ever issued and the id of the newest stored event,
  all before the handshake. Taking the lock has 10 s, and sqlx retries a refused connection within
  them, so a database that refuses connections stops the start after 10 s with `taking the
  database writer lock: the database did not answer in time: taking the writer lock`. The whole
  sequence has 45 s, because the hub kills a monitor that has not answered in 60 s. The monitor does
  not call SEI before the handshake.
- Exit code 0 is a clean stop (SIGTERM or SIGINT). When the monitor exits 1 because of a
  configuration error, a failed start-up step or a lost writer lock, it writes exactly one JSON
  `fatal` line at `error` level, whose `error` field holds the configuration problems, names the
  start-up step that failed, or starts with `the database lock was lost`. A run outside the hub and
  a failure to start logging or the runtime also exit 1, without a `fatal` line. The hub restarts
  the monitor with a back-off.
- After the handshake the process exits only on a signal, a lost writer lock or an error of the
  SDK's `serve` (a `fatal` line and exit 1). An unknown agency or module, a failing cycle, a
  rejected login and a panic in the scheduler leave it running, with the reason in `Status`,
  because an exit would make the hub restart it in a loop.
- A query on the lock's connection every 0.5 s, with 30 s to answer, notices a lost lock (see
  Persistence). The reason after `the database lock was lost: ` is `the lock's connection failed:
  ...`, `the lock's connection did not answer within 30s` or `the lock's connection was replaced`,
  and it never carries the address. A lock lost before the migrations stops the start the same
  way, under the step `applying the database migrations`, and nothing is migrated.
- A second process on the same database fails at start-up with `another sei-monitor process holds
  the database lock`, migrates nothing and does not call SEI.
- On SIGTERM the running cycle is cancelled at once and recorded as `context canceled`. The SDK
  drains the server for at most 1 s.
- `Describe` names the monitor `sei` (`SEI`) with eight kinds. `account_login_failed` and
  `return_overdue` are urgent, the rest are info.
- `Status`, first match wins: hub ahead (below), `FAILING`; an unknown agency or module while
  preparing, or a panicked scheduler, `FAILING` with the text; a cycle running, `CHECKING`; paused,
  `PAUSED` (below); the last cycle failed, `FAILING` with its error (the same text as
  `cycle_runs.error`); account not known yet, `FAILING` with `sei-monitor has not reached SEI
  yet[: <error>]`; the database does not answer within 10 s or the last `Events` read failed,
  `FAILING` with `the database is unavailable`; otherwise `IDLE`. Messages are cut at 500
  characters. `last_success_at` is the last cycle run that finished without an error.
- `Check` starts a cycle, or refuses with Go's texts: not reached SEI yet, already running, less
  than 5 minutes since the last start, or paused.
- `Events(after, limit)` returns the events of the current account with an id above `after`, in
  order, exactly `min(limit, available)` of them, except during the first read. A `limit` of 0 or
  less means 100, and the maximum is 500. `seq` and `id` are `events.id`. Each call acknowledges
  `after` in memory (it never moves back and is not stored). Before the account is known `Events`
  answers `UNAVAILABLE`, and so does a database error (`the database is unavailable`).
- Against a database that refuses new connections while the lock's connection still answers,
  sqlx keeps retrying for the 10 s allowed to get a connection. `Status` and `Events` then take up
  to 10 s to answer, under the hub's 30 s limit per call. A cycle takes about 20 s and shows
  `CHECKING` until it ends, because starting its cycle run waits those 10 s and recording its
  failure waits 10 s more. `Status` then shows `the database did not answer in time: getting a
  connection`. A server that stops ends the lock's connection too, and the monitor exits 1 within
  about a second.
- The first read stops at the highest position issued before start-up. Until one `Events` call
  returns a page shorter than its limit, newer events are left out. The hub stores its first read
  of a monitor as a baseline and announces none of it, so this keeps the first cycle's events out of
  the baseline. They go out on the next read: when the first read ends, the monitor calls
  `events_recorded` with the id of the newest stored event, so the hub reads them at once. The limit
  belongs to one process: a restart before the hub's first read ended lets the new baseline take
  those events in.
- When `after` is above the highest position this database ever issued, the monitor's database was
  reset and the hub's position was not. `Events` then returns nothing and acknowledges nothing,
  logs `the hub is ahead of this database: reset the hub's position for the sei monitor` once, and
  `Status` shows that text until the two agree.
- The monitor serves `Watch` with the SDK's `serve`, given the `Notifier`. `Monitor` owns the
  `Notifier` and hands a clone to its `Store`, and start-up sets it to the id of the newest stored
  event. `Store::apply_cycle` writes every event (a cycle's, and the `login:` and `login-held:`
  ones), and right after a commit that inserted events it calls `events_recorded` with the highest
  inserted id. A failed commit, or a write that inserted nothing, signals nothing. The monitor
  calls `status_changed` when what `Status` answers changes: the next check time is set, a
  preparation fails (an unknown agency or module only when its text changes) or works, the monitor
  pauses, holds or leaves the hold, the scheduler panics, the hub falls ahead of the database or
  back in line, and the last `Events` read fails or recovers. A cycle's start and end signal
  through `status_scope`, which the `RunningGuard` holds, so a cancelled cycle signals its end too.
  The end of the first read calls `events_recorded` with the id of the newest stored event. Signals
  carry the id of a stored event, never the sequence's own value: an insert that meets the
  `(account, kind, ref)` conflict still uses up a sequence value, and a signal above every stored event would
  make the hub read `Events` on every signal until the next event. The first-read boundary and the
  hub-ahead check keep using the highest position issued. A signal is a value in a channel inside
  the process: the hub answers it with `Describe`, `Status` and maybe `Events`, never `Check`, so
  no signal calls SEI or logs in.
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
monitor on its own (after a lost lock, for example), so a restart is not the owner's act and must
not cost a login. A requested `Check` is the only retry. As soon as SEI accepts its login, the
reason is cleared in memory and its database write starts, so a later failure in that cycle follows
the ordinary back-off with the session it has. The clear stays pending until that write lands: a
write that fails, or is cut off because the rest of the cycle failed first, is tried again after
each successful cycle. A restart before it lands starts held again, without a login. A rejection
recorded while the clear is pending drops it. Only a check whose login got no verdict (5xx, a
time-out, a page that is not the API's) goes back to held.

## Pruning

With `RETENTION_DAYS` above 0, every successful cycle deletes, in its own transaction, the events
detected before `start - RETENTION_DAYS` (computed in UTC) and the processes that left before then.
Two locks guard the events:

1. Never delete an event with an id above the hub's acknowledged position. That position lives in
   memory and starts at 0, so nothing is pruned after a restart until the hub reads again.
2. An acknowledged event stays until the cutoff, so a `return_due_soon` is not deleted early and
   created again with a new id.

## Persistence

- The monitor reaches Postgres through SeaORM on sqlx. The tables are hand-written entities in
  `src/store/entity/`. The migrations are `sea-orm-migration` modules in `src/migration/`, run in
  the order of `Migrator::migrations`, and `seaql_migrations` records the ones applied.
- The first migration, `m20261118_000001_goose_schema`, adopts the schema that goose built for 2.1
  and the Go release. It reads `goose_db_version` in the current schema and runs the Up section of
  each `sql/schema/` file that table has not recorded. A database without the table is new and
  gets every file. It refuses a gap (`migration version 1 is not applied but the database is
  already at version 2`) and a table with no rows, accepts versions above 2, and never writes a
  goose row.
- The second, `m20261118_000002_drop_go_tables`, drops what the Go release left in that same
  schema: `deliveries`, `goose_db_version` and every table whose name starts with `whatsmeow_` (the
  Go release's WhatsApp session, keys included). It never uses `CASCADE`. The first two get a
  statement each, and the `whatsmeow_*` tables share one because their foreign keys point at each
  other. If another object depends on one of them, the drop fails, nothing is dropped, and
  start-up stops with Postgres's error and its detail, which names the object.
- 2.1 and the Go release no longer run on a database that went through it. Starting 2.1 on such a
  database fails with `relation "accounts" already exists` and changes no data. It does leave a
  new `goose_db_version` table with one version-0 row, which 2.2 ignores and never drops, because
  the drop migration is already recorded. Drop it by hand.
- **Upgrading from 2.1.** The first 2.2 start creates `seaql_migrations` and drops the Go tables.
  The monitor's role therefore needs `CREATE` on the current schema and must own those tables,
  which 2.1 never needed after its own first start. Without `CREATE` the start fails with
  `applying the database migrations: db error: ERROR: permission denied for schema public`, and
  without ownership with `... must be owner of table deliveries`. Either way it changes nothing,
  but under the hub it becomes a restart loop. No try gets as far as calling SEI. Before the first
  2.2 start, run these as the monitor's role:

  ```sql
  SELECT has_schema_privilege(current_user, current_schema(), 'CREATE');
  SELECT tablename, tableowner FROM pg_tables
  WHERE schemaname = current_schema()
    AND (tablename IN ('deliveries', 'goose_db_version') OR starts_with(tablename, 'whatsmeow_'));
  ```

  The first must return `t`, and every row of the second must show the monitor's role as
  `tableowner`. Later starts need the rights on the tables and on `events_id_seq` that 2.1 needed,
  and `SELECT` on `seaql_migrations`. Without that, the start reads the migrations as pending,
  tries to create the table and fails with the same `permission denied for schema public`, which
  points at the wrong right. It happens when another role ran the first 2.2 start: grant it. Take a `pg_dump` of the database before that start, because once 2.2 has run, 2.1 cannot
  use the database again. A login rejection that 2.1 recorded still holds the first 2.2 start,
  which makes no login.
- A schema change is a new module in `src/migration/` and a new entry at the end of
  `Migrator::migrations`. **Never edit a migration that already exists, nor the files in
  `sql/schema/`**: they are the first migration's input.
- Start-up runs the migrations on the writer lock's connection, in one transaction that first
  checks that the connection still holds the lock. Each migration's own transaction becomes a
  savepoint inside it, and a run that fails records nothing. No migration can leave that
  transaction: `use_transaction(Some(false))` does not take it out, and a statement Postgres
  refuses inside a transaction block, such as `CREATE INDEX CONCURRENTLY`, makes start-up fail.
- When nothing is pending, start-up reads `seaql_migrations` without creating it and sends no
  DDL, so a role with rights on the tables alone starts on a migrated database.
- The SQL lives in the store (`src/store/`) and in the migrations (`src/migration/`, which reads
  `sql/schema/`). Nothing inserts an event outside `Store::apply_cycle`, which locks the account
  row first (`FOR NO KEY UPDATE`), so the order of ids is the order of the commits. Events are
  stored once per `(account, kind, ref)`, so a changed `ref` format lets an event the monitor
  detects again be stored and announced a second time. Never insert explicit ids: the columns are
  `GENERATED ALWAYS AS IDENTITY`.
- Most queries go through SeaORM's query builder. These keep 2.1's SQL text and run as raw
  statements:
  - `ENSURE_ACCOUNT`, whose `ON CONFLICT ... DO UPDATE ... RETURNING *` also gives back an account
    that already exists;
  - `SET_PAUSED_REASON`, `LIST_ACCOUNT_PROCESSES` and `START_CYCLE_RUN`, which the scheduler tests
    find in `pg_stat_activity` by their text;
  - `LOCK_ACCOUNT`, the `FOR NO KEY UPDATE` above;
  - `UPSERT_PROCESS`, whose `DO UPDATE` list leaves `first_seen_at` alone and clears `left_at`;
  - `LAST_ISSUED_SEQ` and `NEWEST_EVENT_ID`, which read the `events_id_seq` sequence and the
    highest id;
  - `PRUNE_CYCLE_RUNS`, a subquery with `OFFSET`.
- A server error prints the way tokio-postgres printed it in 2.1, `db error: ERROR: <message>`
  with `DETAIL` and `HINT` lines when the server sends them, so `Status` and `cycle_runs.error`
  read the same. The severity word is the English one (`ERROR`, `FATAL`) whatever the server's
  `lc_messages`, where 2.1 showed the server's own word. The message follows `lc_messages`.
- The pool has 10 connections. Each one runs with a 20 s `statement_timeout` and a 5 s
  `lock_timeout`, asks the server for TCP keepalives (`tcp_keepalives_idle=30`,
  `tcp_keepalives_interval=10`, `tcp_keepalives_count=3`, `tcp_user_timeout=60000`, in ms), and
  is tested before each use. Getting a connection has 10 s. The pool keeps no minimum, idle limit
  or lifetime, so sqlx runs no maintenance task, `Store::new` works outside a runtime, and an open
  connection stays open.
- The writer lock (`pg_try_advisory_lock`) has a connection of its own, in a one-connection pool
  apart from the store's, with the same keepalives, no statement or lock timeout because the
  migrations run there, and `client_min_messages=warning`. It is never tested before use: a
  failed test would quietly open a new connection without the lock. Every 0.5 s, a query with
  30 s to answer checks in `pg_locks` that this connection still holds the lock. sqlx pings the
  connection each time it goes back to that pool, with no limit of its own, so the wait for the
  connection gets the same 30 s. The limit is that long because a false loss stops the process,
  and the restart logs in to SEI unless a recorded rejection holds it. The monitor gives up on a
  silent connection after about 30.5 s, and the server frees a vanished monitor's lock only after
  about 60 s, so the lock cannot pass to a second copy while the first could still write.

The database layer adds these lines to the log. None carries the address or a password.

- At start-up, when a migration is pending, sea-orm-migration logs `Applying all pending
  migrations`, then `Applying migration '<name>'` and `Migration '<name>' has been applied` for
  each one it runs. Those lines show even when a later migration fails and the run records
  nothing. The monitor then logs `database migrations applied` with the names it applied. On a
  restart that line comes alone, with `[]`. Postgres's notices, such as the migration table's
  `already exists, skipping`, stay out because of `client_min_messages=warning`; warnings and
  errors still show.
- `ping on idle connection returned error` (`info`, sqlx) appears each time the pool replaces a
  connection the server closed while it was idle (`idle_session_timeout`, a server restart,
  `pg_terminate_backend`). There is one line per replaced connection, and it has no address.
- `there is no transaction in progress` (`warn`, `sqlx::postgres::notice`) follows a commit that
  Postgres refused, because SeaORM sends a `ROLLBACK` after it.
- `error occurred while testing the connection on-release` (`warn`) appears when a call is
  dropped in the middle of a statement (the 10 s limit of `Status`, a cancelled clear of the pause
  reason, a stop) or a connection is cut. sqlx then closes that connection.
- `acquired connection, but time to acquire exceeded slow threshold` (`warn`) appears when getting
  a connection took more than 2 s.
