# sei-monitor

sei-monitor watches your processes on SEI. It logs in with your own SEI account through SEI's REST
module (`mod-wssei`), checks the processes of your units every few minutes and records what
changed as events.

It is a plugin for monitor-hub and does not run on its own. The hub starts it, reads its events
and sends the notifications through the hub's channels (email, Telegram, ntfy, webhook, AMQP,
WhatsApp, the feed). The plugin keeps its data in its own Postgres database and talks only to your
SEI and to that database.

Release `1.0.0` was a standalone service with its own notifications, feed, API and web page. That
code is gone. Its image, `ghcr.io/itztas/sei-monitor:1.0.0`, is still the way back (see
[Switching between the Go release and the plugin](#switching-between-the-go-release-and-the-plugin)).

## What it reports

| Event | When |
|---|---|
| `process_entered` | A process shows up in one of your units. |
| `process_left` | A process leaves your units. |
| `activity_added` | A process gets a new activity (andamento). |
| `document_added` | A process gets a new document. |
| `assignment_changed` | A process is assigned to you, to someone else or to nobody. |
| `return_due_soon` | A scheduled return (retorno programado) is due within `RETURN_WARNING_DAYS`. |
| `return_overdue` | A scheduled return is overdue. |
| `account_login_failed` | SEI refused the login, so monitoring paused (see [When SEI refuses the login](#when-sei-refuses-the-login)). |

`account_login_failed` and `return_overdue` reach the hub as urgent, the rest as info.

The first check of a new account only takes note of what is already there. You don't get a
message for every process you had before; reports start with the second check.

## How monitor-hub runs it

The image `ghcr.io/itztas/sei-monitor` holds two files: `/plugin/plugin.json` and the static binary
`/plugin/sei-monitor`, built for amd64 and arm64. The hub's `plugins.json` names the image, and the
hub's image build copies `/plugin/` to `/opt/monitor-hub/plugins/sei/`. The manifest tells the hub
to run `sei-monitor plugin`:

```json
{"protocol_version":1,"name":"sei","command":["sei-monitor","plugin"],"apk":[]}
```

The plugin needs no Alpine package. It carries its own TLS roots and time zone table.

Set the plugin's variables in the hub's `.env` with the `SEI__` prefix: `SEI__SEI_URL`,
`SEI__DATABASE_URL` and so on. The hub gives the plugin only the variables with that prefix, with
the prefix removed, plus its own `PATH`, `TZ`, `TMPDIR` and `MONITOR_HUB_DATA_DIR`. A misspelled
name is not an error: the plugin never sees it.

Run the image by hand and it prints that the hub did not launch it, then exits with code 1.

The hub's API shows the plugin in `GET /api/plugins` (state and message) and starts a check with
`POST /api/plugins/sei/check`. A check is refused while one is running and within 5 minutes of the
last one. The hub's own deploy guide (`docs/deploy.md` in the monitor-hub repository) is the full
runbook for moving the old service onto the hub.

## Configuration

| Variable | Default | What it does |
|---|---|---|
| `DATABASE_URL` | | Required, secret. `postgres://user:password@host/database`. The plugin connects without TLS. `DATABASE_URL_FILE` reads it from a file. |
| `SEI_URL` | | Required. SEI address, without `/sei` at the end. Must be https; plain http works only for localhost. |
| `SEI_API_URL` | discovered | Address of the `mod-wssei` API, only if the discovery fails. |
| `SEI_USERNAME` | | Required. Your SEI login. |
| `SEI_PASSWORD`, `_FILE`, `_CMD` | | Required, secret. Set exactly one (see [Passwords](#passwords)). |
| `SEI_ORG` | | Required. Agency acronym or id, as `orgs` lists them. |
| `SEI_UNITS` | your current unit | Unit acronyms separated by commas, or `all`. |
| `SEI_TIMEZONE` | `America/Sao_Paulo` | Time zone of the dates SEI shows. Roraima uses `America/Boa_Vista`. |
| `SEI_CONCURRENCY` | `3` | Simultaneous calls to SEI, from 1 to 10. |
| `POLL_INTERVAL` | `15m` | Time between checks, at least `5m`. |
| `RETURN_WARNING_DAYS` | `2` | How many days before a scheduled return to warn, from 0 to 60. |
| `RETENTION_DAYS` | `90` | How many days to keep events and the processes that left your units, up to 3650. Use at least `RETURN_WARNING_DAYS` + 2, or `0` to keep everything. |
| `NOTIFICATION_DETAIL` | `minimal` | `minimal` or `full`, see [Detail level](#detail-level). |
| `LOG_LEVEL` | `info` | `debug`, `info`, `warn` or `error`. |

A bad value stops the plugin at start-up with one `fatal` line in the hub's log that lists every
problem at once. The hub then restarts it with a back-off until the configuration is fixed.

Keep `SEI_URL`, `SEI_USERNAME` and `SEI_ORG` stable. Together they identify the account in the
database. A change starts a new account, and its first check is a silent baseline.

### Passwords

Set exactly one of these:

- `SEI_PASSWORD`: the simplest, and the least safe. Anyone who can inspect the hub's container can
  read its environment.
- `SEI_PASSWORD_FILE`: the path of a file that holds the password, such as a Docker secret mounted
  into the hub's container. The plugin reads it once, at start-up. The hub runs as user 65532, so
  that user must be able to read the file.
- `SEI_PASSWORD_CMD`: a command that prints the password. The plugin runs it at every login,
  without a shell, with a 30-second limit, and without the secret variables in its environment. Its
  program must exist inside the hub's image, which has no password manager; start-up refuses a
  program it cannot find on `PATH`.

The session token SEI hands out after the login is a credential too. The plugin keeps it only in
memory.

### Detail level

At the `minimal` level, events carry the event type, the process number, the unit and the date.
`full` adds the process type and description, the activity text, the document name and who did
what. Choose it only if you are fine with that content passing through the hub's channels.

Your own SEI login, which is often your CPF, never shows up. Texts say "you", and the event data
carries `assigned_to_me` and `activity_by_me` instead of your acronym.

## Status

The hub polls the plugin every 15 seconds. `GET /api/plugins` shows what the plugin answered:

| State | Message | Meaning |
|---|---|---|
| `checking` | | A check is running. |
| `idle` | | The last check is done. `next_check_at` says when the next one starts. |
| `paused` | the reason, then what to do | SEI refused the login. See below. |
| `failing` | `sei-monitor has not reached SEI yet`, with the error when there is one | The plugin has not reached SEI since it started. It tries again with a back-off. |
| `failing` | an unknown agency or module | `SEI_ORG` or the module does not exist. Fix it and restart the hub. |
| `failing` | `the database is unavailable` | The plugin's database does not answer. Events wait in the database until it does. |
| `failing` | `the hub is ahead of this database: reset the hub's position for the sei plugin` | The plugin's database was reset and the hub's position for `sei` was not. Reset both together. |

`last_success_at` is the end of the last check that finished without an error.

The plugin does not exit on these errors. An exit would make the hub restart it in a loop, and with
a wrong password every restart would cost a rejected login.

## When SEI refuses the login

The plugin tries the login once. When SEI rejects it, the plugin stops logging in, because repeated
failures can lock the account. It stays up, keeps serving its events, and:

- records one `account_login_failed` event (urgent) titled `sei-monitor could not log in to SEI`.
  The body gives SEI's reason, for example `Reason: SEI rejected the login (Usuário ou Senha
  Inválida.)`, and says monitoring is paused until sei-monitor restarts with the configuration
  fixed. That wording comes from the Go release: a restart alone is not enough, see the steps below;
- logs `SEI rejected the login; monitoring is paused until sei-monitor restarts with the
  configuration fixed` at `error`, with the reason;
- answers `paused` with `<reason>; restart the plugin with the configuration fixed, then request a
  check`, and refuses checks.

It also stores the reason in its database. A plugin process that starts with a reason on record is
held: it does not log in at all. It logs `SEI rejected the login in an earlier run; monitoring stays
paused until a check is requested with the configuration fixed`, answers `paused` with `<reason>;
nothing is logging in: request a check once the configuration is fixed`, and records one more
`account_login_failed` event. That event is recorded at most once per UTC day, however often the
plugin restarts.

After a rejected login, a restart alone never logs in again, on purpose. The hub restarts the
plugin by itself, for example after a lost database connection, and every attempt with a wrong
password counts toward SEI locking the account. Only a check you request retries the login. With a
correct password, each plugin restart costs one login.

To resume:

1. Fix the password: in SEI, or in the hub's `.env`, the secret file or the password command.
2. Restart the hub so the plugin starts held with the new configuration. After editing `.env`, run
   `docker compose up -d` in the hub's folder (a plain `restart` keeps the old environment);
   otherwise `docker compose restart monitor-hub`. You can skip this step when the plugin already
   shows the held message and its configuration did not change: you fixed the password in SEI, or
   it comes from `SEI_PASSWORD_CMD`, which runs at every login.
3. Request a check, with the hub's `API_TOKEN` in a shell variable (the hub's deploy guide shows
   how to set it without writing it into the history) and the hub's port:

   ```sh
   curl -s -X POST -H "Authorization: Bearer $API_TOKEN" http://127.0.0.1:8090/api/plugins/sei/check
   ```

   Expect `{"accepted":true,...}`. `sei-monitor has not reached SEI yet` means the plugin is still
   connecting after the restart: ask again in a minute.
4. Read `GET /api/plugins` after half a minute. `sei` should be `idle` with a fresh
   `last_success_at`. If SEI rejected the password again, it is `paused` with the first message,
   and you start over from step 1.

A requested check that fails for another reason, such as SEI being down, leaves the plugin held.
Request another one later. Network errors and SEI outages outside a pause are retried on their
own: after 30 seconds, then a minute, doubling up to the check interval.

## The hub's first read

The first time the hub reads a plugin it does not know, it stores what the plugin already holds as
a baseline and announces none of it. That read comes on the hub's first poll after the plugin has
reached SEI.

The plugin's own first check may find changes before that read happens. So the plugin holds back
everything newer than what it held at start-up until the hub's first read has ended. Those events
reach the hub on the next poll and are announced as usual.

That hold belongs to one plugin process. If the plugin or the hub restarts after the first check
recorded events and before the hub logged `baseline complete` for `sei`, the new process counts
those events as history and the baseline takes them in. Nobody announces them. When that happens,
check by hand:

1. In the hub's log, look for the plugin's `cycle finished` lines from before the restart. An
   `events` count above 0 means that check recorded events.
2. List the events recorded since the hub started (use your role and database names, and the
   start time in UTC):

   ```sql
   SELECT id, kind, payload::jsonb->>'process_number' AS process, detected_at
   FROM events
   WHERE detected_at >= '<when the hub started>'
   ORDER BY id;
   ```

3. Look for each of them in your channels, and pass on the ones that never arrived.

## Operator commands

The binary has three commands for you, besides `plugin`:

| Command | What it does |
|---|---|
| `orgs` | Lists the acronym and id of every agency on the SEI login page. It needs only `SEI_URL` and never logs in. |
| `test-login` | Logs in and shows the SEI and module versions and your units, never the session token. |
| `diagnose` | Checks how the SEI API behaves and prints only counts and types, so its output is safe to share. |

They run with your real account. `test-login` and `diagnose` cost one login each run. `diagnose`
stops at the first rejected login instead of trying the remaining calls.

Run them inside the hub's container, where the binary lives. They read the same variables as the
plugin, without the `SEI__` prefix. This copies the hub's `SEI__` variables to the plain names
inside the container, so no secret shows up on a command line:

```sh
cd /opt/stacks/monitor-hub
docker compose exec monitor-hub sh -c '
for name in SEI_URL SEI_API_URL SEI_USERNAME SEI_PASSWORD SEI_PASSWORD_FILE SEI_PASSWORD_CMD \
  SEI_ORG SEI_UNITS SEI_TIMEZONE SEI_CONCURRENCY; do
  eval "value=\${SEI__$name-}"
  [ -z "$value" ] || export "$name=$value"
done
exec /opt/monitor-hub/plugins/sei/sei-monitor "$@"' sei test-login
```

Put `orgs` or `diagnose` in place of `test-login` at the end. When the hub reads the password from
`SEI__SEI_PASSWORD_FILE` or `SEI__SEI_PASSWORD_CMD`, the command uses the same file or program.
Ctrl+C stops a command with exit code 130.

## Switching between the Go release and the plugin

Both programs use the same database and schema, and neither needs a SQL step to take over from the
other. Neither keeps the other out either: the Go release holds no lock while it runs, and the
plugin's lock only keeps a second plugin out. Running both at once makes two monitors fight over
the same account, so stop one completely before starting the other.

### Releases and `latest`

`v2.0.0` is tagged on the branch head before the merge into `main`, so the tag publishes
`ghcr.io/itztas/sei-monitor:2.0.0` and `:2.0` and leaves `latest` alone. The merge into `main` moves
`latest` to the plugin. The old production compose file names `ghcr.io/itztas/sei-monitor:latest`
directly, and the plugin image exits at once when run alone, which `restart: unless-stopped` turns
into a restart loop with SEI unwatched. So pin the old stack to `ghcr.io/itztas/sei-monitor:1.0.0`
before the merge, and keep it pinned: the rollback needs that image.

### From the Go release to the plugin

1. Pin the old stack to `:1.0.0` (above).
2. Let the Go release send everything it has. It sends pending notifications every 30 seconds and
   retries a failed one after 1 minute, 5 minutes, 30 minutes, 2 hours and 6 hours. In its
   database this must return 0:

   ```sql
   SELECT count(*) FROM deliveries WHERE status = 'pending';
   ```

   A notification still pending at the switch is never sent while the plugin runs: the hub's first
   read takes that event as history.
3. Give the hub the same `SEI_URL`, `SEI_USERNAME` and `SEI_ORG` as the old `.env`, with the
   `SEI__` prefix, and point `SEI__DATABASE_URL` at the old database.
4. Stop the Go release, then start the hub.
5. The plugin reads the database as Go left it: no new account, no new baseline, no repeated
   events. The hub's first read takes the Go-era events as history (see [The hub's first
   read](#the-hubs-first-read)).
6. If the Go release was paused by a rejected login, the plugin starts held. Fix the password and
   request a check (see [When SEI refuses the login](#when-sei-refuses-the-login)).

### Back to the Go release

1. Stop the hub completely.
2. Start `ghcr.io/itztas/sei-monitor:1.0.0` on the same database, with the same `SEI_URL`,
   `SEI_USERNAME` and `SEI_ORG`. If a pause reason is on record, Go tries the login once at start,
   as it always did.

What to expect:

- Go announces only what it detects from then on. It never announces an event the plugin stored,
  even when it detects the same change again, because Go writes a notification row only for the
  events it inserts itself. Its first notification can take up to 30 seconds after the check that
  found it.
- Notifications Go left pending before the switch to the plugin go out right after the rollback,
  possibly days late. Step 2 above avoids that.
- When the plugin pruned old events, Go's notification rows for them went too (`ON DELETE
  CASCADE`), pending ones included. Those are never sent.
- Notifications the hub had not delivered are not sent by anyone. List them in the hub's database
  before you stop it.
- The plugin adds no table or column. A future migration keeps this way back open only while it
  leaves alone the tables, columns, constraints and event formats the Go release uses. Go does not
  refuse a database that holds a migration it does not know, so a breaking change would show up as
  wrong or failing queries.

## Security

- The SEI password, the session token and `DATABASE_URL` print as `[redacted]` everywhere. None of
  them appears in a log, an error, an event, a gRPC answer or command output, and the password is
  never stored.
- On start, the binary turns off core dumps, blocks other processes from reading its memory
  through debugging interfaces, and sets its file mode mask to 077.
- Calls to SEI verify TLS against the root certificates built into the binary (the
  `webpki-root-certs` crate), not the system's. Each
  request has a 30-second total limit, reads at most 16 MiB of a body, and errors never include
  addresses that may carry a token. API calls refuse redirects.
- The image has no shell and no other program, and runs as user 65532. Inside the hub the plugin
  runs as the hub's user, in the hub's hardened container.
- Database connections have a 10-second connect limit, a 20-second statement limit and TCP
  keepalives. The connection to Postgres is not encrypted, so keep the database on the same
  machine or Docker network.
- Everything the plugin keeps lives in Postgres. Treat the database and its backups as secrets: they
  hold your processes and, with `full`, their contents.
- To keep the load on SEI low, checks run at least 5 minutes apart, with a cap on simultaneous
  calls and some random spread between checks.

## Known limitations

- Agencies without `mod-wssei` aren't supported. The plugin doesn't read SEI's web pages, apart
  from the login page that lists the agencies.
- Special follow-ups (acompanhamentos especiais) aren't monitored, because the module's route for
  them fails on at least one SEI instance.
- Suspended processes (sobrestados) disappear from the module's process list, so the plugin
  reports them as having left your units, and as new when they come back.
- Accounts with two-factor authentication are untested.
- One SEI account per plugin.
- A captcha or Cloudflare challenge in front of SEI stops the plugin.
- A list of more than 400 pages for one unit fails the check instead of being cut short.

### Where the plugin differs from the Go release

The plugin was checked against the Go release's own output. These differences are deliberate:

- A login rejected in an earlier run holds the plugin until you request a check. Go tried again at
  every start.
- `diagnose` stops at the first rejected login. Interrupted, it exits with 130 (Go went on and
  exited 0). A user without units and `SEI_UNITS=all` is an error with exit 1.
- `SEI_TIMEZONE=Local`, `Factory` and `posixrules` are refused. The time zone table is the 2025b
  release built into the binary. It differs from Go's for a few zones outside Brazil, none of them
  Brazilian.
- A secret that is not valid UTF-8 (the password, `DATABASE_URL`, their files, the output of the
  password command) is refused without showing it. Go used the raw bytes.
- `SEI_PASSWORD_CMD` runs a program found through a relative `PATH` entry, which Go refused.
- The pruning cutoff is computed in UTC, and never deletes an event the hub has not read.
- SEI answers that the real server never sends (lone surrogate escapes, numbers too large for a
  float, duplicate keys, nesting of 128 levels or more) are refused as `answer outside the API
  format`.
- A login page that is slow to parse fails at the request time limit, and pages in some East Asian
  or UTF-16 encodings decode slightly differently. The real login page is plain ISO-8859-1.
- The database pool keeps up to 10 connections open, where Go closed idle ones after 5 minutes.
- The `User-Agent` is `sei-monitor/<version>`.

## Development

- `make check` runs `cargo fmt --check`, clippy and the tests. `make up` starts a development
  Postgres on `127.0.0.1:5432` from `compose.yaml`; copy `.env.example` to `.env` first and set
  `POSTGRES_PASSWORD`.
- The database tests use `TEST_DATABASE_URL` when it is set, and otherwise start a throwaway
  Postgres from `initdb` and `postgres` on `PATH`. The compatibility tests build the Go release from
  the tag `v1.0.0` with `go`. With `CI` set, a missing tool fails the tests instead of skipping them.
- `git config core.hooksPath .githooks` turns on the hooks: Conventional Commits messages, and
  `cargo fmt --check` plus the tests before each commit.
- No test talks to a real SEI. A fake server in `tests/support/` plays its part.
- To run the operator commands from a checkout, export the plain variables and run
  `cargo run -- test-login`. The binary never reads `.env`.

## License

The license hasn't been chosen yet.
