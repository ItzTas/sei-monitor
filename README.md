# sei-monitor

sei-monitor watches your processes on SEI. It logs in with your own SEI account through SEI's REST
module (`mod-wssei`), checks the processes of your units every few minutes and records what
changed as events.

It is a plugin for monitor-hub and does not run on its own. The hub starts it, reads its events
and sends the notifications through the hub's channels (email, Telegram, ntfy, webhook, AMQP,
WhatsApp, the feed). The plugin keeps its data in its own Postgres database and talks only to your
SEI and to that database.

Release `1.0.0` was a standalone service with its own notifications, feed, API and web page. That
code is gone (see [Switching from the Go release to the plugin](#switching-from-the-go-release-to-the-plugin)).

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

## How the plugin tells the hub to read

The hub keeps a `Watch` call open to the plugin. The plugin sends a signal on that call when the hub
should read it, and the hub then reads the plugin's state, and its events when there are new ones.
The plugin signals when:

- a check starts or ends, or the time of the next check changes;
- SEI refuses the login, or the plugin starts held by a refusal from an earlier run;
- a check you requested ends the hold, or SEI gave its login no answer and the plugin is held again;
- the plugin cannot reach SEI, or does not find the agency or the module, and when it reaches SEI;
- the hub is ahead of the plugin's database, and when the two agree again;
- the hub cannot read the plugin's events, and when it can again;
- the plugin's scheduler stops after an internal error;
- the plugin records an event;
- the hub's first read ends (see [The hub's first read](#the-hubs-first-read)).

A signal never contacts SEI and never logs in. The hub answers it by reading what the plugin holds
in memory and in its database. It never starts a check because of a signal.

When 30 seconds pass without a signal, the plugin sends one anyway, so the hub knows the call is
alive. If the call drops, the hub reads the plugin every 15 seconds until it opens the call again. A
hub without `Watch` never opens the call and reads the plugin every 15 seconds.

## Configuration

| Variable | Default | What it does |
|---|---|---|
| `DATABASE_URL` | | Required, secret. `postgres://user:password@host:port/database?sslmode=disable`, or the same with `postgresql://`. The password is required, a URL with several hosts is refused, and the plugin connects without TLS. `DATABASE_URL_FILE` reads it from a file. |
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

The plugin is stricter about its database than the Go release was. Point `DATABASE_URL` straight
at Postgres, not at a transaction pooler, and put the host, port, user, password and database in
it. The plugin checks the address when it takes its database lock, and a bad one stops the start
with a message that never shows the address:

- With no password, or an empty one: `DATABASE_URL has no password: add the role's password to
  the URL`. If the database trusts the role without a password, put any password there; Postgres
  ignores it.
- With several hosts, no host, or a query parameter the plugin does not know: `DATABASE_URL is not
  a valid Postgres address`. The libpq parameters that 2.1 accepted, such as `connect_timeout` and
  `keepalives`, are accepted and ignored.

Use `sslmode=disable` or `prefer`. The plugin has no TLS, so `require` fails.

When the connection that holds the plugin's lock closes, or gives no answer for 30 seconds, the
plugin exits, the hub starts it again, and each new start logs in to SEI once, up to 12 times an
hour. So keep anything outside the plugin from ending its database connections, such as a firewall
that cuts long-lived connections or a script that runs `pg_terminate_backend`. The role must own
the tables and be able to read the `events_id_seq` sequence.

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
  without a shell, with a 30-second limit. Its environment leaves out the secret variables: the
  passwords, tokens and database addresses of the plugin and of the Go release, the same names
  behind a prefix such as `SEI__` or `DIARIO__`, and `AMQP_URL`. Its program must exist inside the
  hub's image, which has no password manager; start-up refuses a program it cannot find on `PATH`.

A password with `$` or quotes in the hub's `.env` can reach the plugin changed by the way that file
is read. That costs one rejected login and a pause, so look at `GET /api/plugins` right after the
first start.

The session token SEI hands out after the login is a credential too. The plugin keeps it only in
memory.

### Detail level

At the `minimal` level, events carry the event type, the process number, the unit, the date and
the link to the process (see [Known limitations](#known-limitations)).
`full` adds the process type and description, the activity text, the document name and who did
what. Choose it only if you are fine with that content passing through the hub's channels.

Your own SEI login, which is often your CPF, never shows up. Texts say "you", and the event data
carries `assigned_to_me` and `activity_by_me` instead of your acronym.

## Status

The plugin tells the hub when its state changes or it records an event, and the hub reads it right
away (see [How the plugin tells the hub to read](#how-the-plugin-tells-the-hub-to-read)). The hub
also reads it every 5 minutes. A hub without `Watch` reads it every 15 seconds instead.
`GET /api/plugins` shows what the plugin answered:

| State | Message | Meaning |
|---|---|---|
| `checking` | | A check is running. |
| `idle` | | The last check worked. `next_check_at` says when the next one starts. |
| `paused` | the reason, then what to do | SEI refused the login. See below. |
| `failing` | the error of the last check | The last check failed: SEI answered with an error or with a page that is not the API's, a unit in `SEI_UNITS` is no longer yours, the password command failed, or the database refused what the check found. Nothing is recorded while this lasts. The plugin tries again after 30 seconds, then a minute, doubling up to the check interval, and goes back to `idle` after the first check that works. |
| `failing` | `sei-monitor has not reached SEI yet`, with the error when there is one | The plugin has not reached SEI since it started. It tries again with the same back-off. |
| `failing` | an unknown agency or module | The login page does not list `SEI_ORG`, or neither module address answered like the module. A maintenance or error page in front of SEI looks the same, so the plugin keeps trying with the same back-off; these tries never log in. If the message stays, fix `SEI_ORG` or set `SEI_API_URL`, then restart the hub. |
| `failing` | `the database is unavailable` | The plugin's database does not answer, or the last read of the events failed. Events wait in the database until it answers. |
| `failing` | `the hub is ahead of this database: reset the hub's position for the sei plugin` | The plugin's database was reset and the hub's position for `sei` was not. Reset both together. |
| `failing` | `the monitor stopped after an internal error; see the plugin log` | A bug stopped the plugin's scheduler. Restart the hub and send the plugin log. |

`last_success_at` is the end of the last check that finished without an error.

The plugin does not exit on these errors. An exit would make the hub restart it in a loop, and with
a wrong password every restart would cost a rejected login.

When the database refuses connections, the plugin waits 10 seconds for it before it answers a read
from the hub, and a check takes about 20 seconds to fail. A plugin that starts while the database
refuses connections gives up after 10 seconds, and the hub tries again later.

Nobody is told when the plugin is `failing`, or stays `paused` without a new event: the hub sends no
notification about a plugin's state. Look at `GET /api/plugins` after every hub restart and every
configuration change.

## When SEI refuses the login

The plugin tries the login once. When SEI rejects it, the plugin stops logging in, because repeated
failures can lock the account. It stays up, keeps serving its events, and:

- records one `account_login_failed` event (urgent) titled `sei-monitor could not log in to SEI`.
  The body gives SEI's reason, for example `Reason: SEI rejected the login (Usuário ou Senha
  Inválida.)`, and ends with `Fix the configuration, restart the hub if you changed its settings,
  then request a check for the sei plugin.`;
- logs `SEI rejected the login; monitoring is paused until sei-monitor restarts with the
  configuration fixed` at `error`, with the reason. That line keeps the Go release's wording: a
  restart alone is not enough, see the steps below;
- answers `paused` with `<reason>; restart the plugin with the configuration fixed, then request a
  check`, and refuses checks.

It also stores the reason in its database. A plugin process that starts with a reason on record is
held: it does not log in at all. It logs `SEI rejected the login in an earlier run; monitoring stays
paused until a check is requested with the configuration fixed`, answers `paused` with `<reason>;
nothing is logging in: request a check once the configuration is fixed`, and records one more
`account_login_failed` event. Its reason ends with `still paused after a restart: request a check
once the configuration is fixed`, followed by the same last sentence as the first event. That event
is recorded at most once per UTC day, however often the plugin restarts.

In both cases the plugin signals the hub, so the `account_login_failed` event and the `paused`
state reach the hub right away. A hub without `Watch` sees them on its next poll, within 15 seconds.

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

Once SEI accepts the login of a requested check, the plugin drops the recorded rejection and
erases it from its database, even when something later in that check fails. The plugin then shows
`failing` with the error and tries again on its own after 30 seconds, with the session it already
has. If the database did not take the erase, the plugin writes it again after its next check that
succeeds. A restart before that finds the rejection still recorded and holds the plugin again,
without a login; request another check. Only a check whose login got no answer from SEI (an error
page, a time-out) leaves the plugin held; request another one later. Network errors and SEI
outages outside a pause are retried on their own: after 30 seconds, then a minute, doubling up to
the check interval.

## The hub's first read

The first time the hub reads a plugin it does not know, it stores what the plugin already holds as
a baseline and announces none of it. That read comes right after the plugin has reached SEI,
because the plugin signals the hub then. A hub without `Watch` makes it on its first poll after
that.

The plugin's own first check may find changes before that read happens. So the plugin holds back
everything newer than what it held at start-up until the hub's first read has ended. When that read
ends, the plugin signals the hub again, and the events it held back reach the hub right away and are
announced as usual. A hub without `Watch` gets them on its next poll, within 15 seconds.

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

They run with your real account, also while the plugin is running. `test-login` and `diagnose`
cost one login each run and don't look at a rejection the plugin recorded, so with a wrong password
each run is one more rejected login. `diagnose` stops at the first rejected login instead of trying
the remaining calls.

Run them inside the hub's container, where the binary lives. They read the same variables as the
plugin, without the `SEI__` prefix. This copies the hub's `SEI__` variables to the plain names
inside the container and removes every other variable except `PATH`, so the command, and a
password command it runs, never see the hub's other settings. No secret shows up on a command
line:

```sh
cd /opt/stacks/monitor-hub
docker compose exec monitor-hub sh -c '
names="SEI_URL SEI_API_URL SEI_USERNAME SEI_PASSWORD SEI_PASSWORD_FILE SEI_PASSWORD_CMD SEI_ORG SEI_UNITS SEI_TIMEZONE SEI_CONCURRENCY"
for name in $names; do
  eval "value=\${SEI__$name-}"
  [ -z "$value" ] || export "$name=$value"
done
for name in $(env | sed -n "s/^\([A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p"); do
  case " PATH $names " in *" $name "*) ;; *) unset "$name" ;; esac
done
exec /opt/monitor-hub/plugins/sei/sei-monitor "$@"' sei test-login
```

Put `orgs` or `diagnose` in place of `test-login` at the end. When the hub reads the password from
`SEI__SEI_PASSWORD_FILE` or `SEI__SEI_PASSWORD_CMD`, the command uses the same file or program.
Ctrl+C stops a command with exit code 130.

## Switching from the Go release to the plugin

The plugin takes over the Go release's database and keeps its accounts, processes and events. It
needs no SQL step when its role can create tables and owns the Go release's tables (step 3 below).
The Go release holds no lock while it runs, and the plugin's lock only keeps a second plugin out.
Running both at once makes two monitors fight over the same account, so stop the Go release
completely before starting the plugin.

### Releases and `latest`

`v2.0.0` is tagged on the branch head before the merge into `main`, so the tag publishes
`ghcr.io/itztas/sei-monitor:2.0.0` and `:2.0` and leaves `latest` alone. The merge into `main` moves
`latest` to the plugin. The old production compose file names `ghcr.io/itztas/sei-monitor:latest`
directly, and the plugin image exits at once when run alone, which `restart: unless-stopped` turns
into a restart loop with SEI unwatched. So pin the old stack to `ghcr.io/itztas/sei-monitor:1.0.0`
before the merge.

### Steps

1. Pin the old stack to `:1.0.0` (above).
2. Let the Go release send everything it has. It sends pending notifications every 30 seconds and
   retries a failed one after 1 minute, 5 minutes, 30 minutes, 2 hours and 6 hours. In its
   database this must return 0:

   ```sql
   SELECT count(*) FROM deliveries WHERE status = 'pending';
   ```

   A notification still pending at the switch is never sent: the plugin's first start deletes it
   with the `deliveries` table, and the hub's first read takes its event as history. If a channel
   never delivers (an unpaired WhatsApp number, for example), its rows stay pending: pair it first.

   In the same database this must return an empty reason:

   ```sql
   SELECT paused_reason FROM accounts;
   ```

   With a reason there, the plugin starts held and watches nothing until you request a check. It
   says so once, in an event the hub announces on its second read.
3. Check the role and back the database up. The plugin's first start drops `deliveries`,
   `goose_db_version` and every `whatsmeow_*` table, the Go release's WhatsApp session with it, so
   the Go release can no longer run on this database. The plugin's role needs `CREATE` on the
   schema and must own those tables: run the checks in [Upgrading from 2.1](#upgrading-from-21) as
   that role, then take a `pg_dump` of the database.
4. Give the hub the same `SEI_URL`, `SEI_USERNAME` and `SEI_ORG` as the old `.env`, with the
   `SEI__` prefix, and point `SEI__DATABASE_URL` at the old database.
5. Stop the Go release, then start the hub. Go keeps checking SEI until it stops, and events it
   detects between step 2 and the stop are never announced, so do both close together.
6. The plugin reads the database as Go left it: no new account, no new baseline, no repeated
   events. The hub's first read takes the Go-era events as history (see [The hub's first
   read](#the-hubs-first-read)).
7. If the Go release was paused by a rejected login, the plugin starts held. Fix the password and
   request a check (see [When SEI refuses the login](#when-sei-refuses-the-login)).
8. After the first start, `GET /api/plugins` shows `sei` as `idle` with a fresh
   `last_success_at`, `SELECT count(*) FROM accounts` still returns 1, and the hub logs
   `baseline complete` for `sei`. The baseline comes on the hub's first poll after the plugin has
   reached SEI. Don't restart the hub before that line (see [The hub's first
   read](#the-hubs-first-read)).

## Upgrading from 2.1

The first start of 2.2 on a 2.1 database keeps every account, process, document, event and cycle
run, with the same ids. It drops the tables the Go release left behind: `deliveries`,
`goose_db_version` and every `whatsmeow_*` table. A login rejection that 2.1 recorded still holds
the plugin, and nothing logs in. The hub's log shows `database migrations applied` with the two
migration names on that start, and with `[]` on every later one.

To create its `seaql_migrations` table and drop those tables, the plugin's role needs `CREATE` on
its schema and must own the tables it drops. 2.1 never needed either after its own first start.
Without them the start stops with `permission denied for schema public` or `must be owner of table
deliveries` and changes nothing, and the hub restarts the plugin in a loop until you fix the role.
Those tries never reach SEI. Run these as the plugin's role first:

```sql
SELECT has_schema_privilege(current_user, current_schema(), 'CREATE');
SELECT tablename, tableowner FROM pg_tables
WHERE schemaname = current_schema()
  AND (tablename IN ('deliveries', 'goose_db_version') OR starts_with(tablename, 'whatsmeow_'));
```

The first must return `t`, and every row of the second must show the plugin's role as
`tableowner`. Later starts need only the rights 2.1 needed.

Take a `pg_dump` of the database before that first start. Once 2.2 has run on it, neither 2.1 nor
the Go release can use it again. 2.1 stops with `relation "accounts" already exists` and changes no
data, but it leaves a new `goose_db_version` table behind; drop it by hand.

## Security

- The SEI password, the session token and `DATABASE_URL` print as `[redacted]` everywhere. None of
  them appears in a log, an error, an event, a gRPC answer or command output, and the password is
  never stored.
- On start, the binary turns off core dumps, blocks other processes from reading its memory
  through debugging interfaces, and sets its file mode mask to 077.
- Calls to SEI verify TLS against the root certificates built into the binary (the
  `webpki-root-certs` crate), not the system's. Each request has a 30-second total limit, reads at
  most 16 MiB of a body, and errors never include addresses that may carry a token. API calls
  refuse redirects.
- The image has no shell and no other program, and runs as user 65532. Inside the hub the plugin
  runs as the hub's user, in the hub's hardened container.
- Getting a database connection has a 10-second limit, and each query of the store 20 seconds. The
  plugin asks Postgres to send TCP keepalives on its connections, and it checks the connection that
  holds its lock every half second. The connection to Postgres is not encrypted, so keep the
  database on the same machine or Docker network.
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
- A captcha or Cloudflare challenge in front of SEI fails every check that meets it. The plugin
  shows `failing` and keeps trying with the back-off. A challenge on the login route makes each
  of those tries post the password again, so watch `GET /api/plugins` if SEI puts one up.
- Each event carries a link to its process, built from `SEI_URL` and the process id:
  `{SEI_URL}/sei/controlador.php?acao=procedimento_trabalhar&id_procedimento=<id>`. It is the Go
  release's feed link and holds no session, so SEI may ask you to log in first or refuse it. It
  was never opened against a real SEI. It is wrong when your SEI does not live under `/sei/`, as
  can happen when you set `SEI_API_URL`. It goes out at the `minimal` level too and shows your SEI
  address and SEI's internal process id.
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
- The pruning cutoff is computed in UTC, and never deletes an event the hub has not read.
- The `account_login_failed` event ends with how to resume instead of Go's sentence that a
  restart resumes monitoring.
- A NUL character in text from SEI is dropped where the text enters. In Go it failed every check
  and stored nothing.
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
  Postgres from `initdb` and `postgres` on `PATH`, on a local TCP port. To use the compose
  Postgres, export `TEST_DATABASE_URL` in your shell, with the same password as
  `POSTGRES_PASSWORD`; that URL needs a password, as `DATABASE_URL` does. The conformance test
  runs the program named by `PLUGIN_CONFORMANCE`, or else installs `plugin-conformance` with `go`.
  With `CI` set, a missing tool fails the tests instead of skipping them.
- `git config core.hooksPath .githooks` turns on the hooks: Conventional Commits messages, and
  `cargo fmt --check` plus the tests before each commit. The hook and `make` run cargo through
  `rustup run` with the toolchain in `rust-toolchain.toml` when `rustup` is installed, so another
  cargo earlier on `PATH` is never used by accident.
- No test talks to a real SEI. A fake server in `tests/support/` plays its part.
- To run the operator commands from a checkout, export the plain variables and run
  `cargo run -- test-login`. The binary never reads `.env`.

## License

The license hasn't been chosen yet.
