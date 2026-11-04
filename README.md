# sei-monitor

sei-monitor watches your processes on SEI and tells you when something changes. It logs in with
your own SEI account through SEI's REST module (`mod-wssei`), checks the processes of your units
every few minutes and turns what changed into events. You can read them in an RSS feed or a JSON
API, or get them by email, Telegram, ntfy, webhook or WhatsApp.

It is self-hosted. It runs on your own machine or server, keeps its data in SQLite or Postgres, and
talks only to your SEI and to the channels you turn on. The name is provisional.

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

The very first check only takes note of what is already there. You won't get a message for every
process you had before you installed sei-monitor; reports start with the second check.

## Before you start

- Your SEI needs the `mod-wssei` module turned on. The `orgs` and `test-login` commands tell you
  whether it answers.
- Docker with Compose, or Go 1.27 to build from source.

## Quick start with Docker

1. Copy the example configuration: `cp .env.example .env`.
2. Set `SEI_URL` and list the agencies to find yours:
   `docker compose run --rm sei-monitor orgs`.
3. Fill in `SEI_USERNAME`, `SEI_ORG` and the password (see [Passwords](#passwords)).
4. Check the login: `docker compose run --rm sei-monitor test-login`. It shows the SEI and module
   versions and your units, and never the session token.
5. Create the tokens for the feed and the API, running the command once for each:
   `docker compose run --rm sei-monitor generate-token`.
6. Turn on the channels you want in `.env` and try them:
   `docker compose run --rm sei-monitor send-test`.
7. Start it: `docker compose up -d`, and follow the logs with `docker compose logs -f sei-monitor`.

Compose builds the image from this repository and keeps the SQLite database in the `data` volume.
To use Postgres instead, follow the comments next to `DATABASE_URL` in `.env.example` and start
with `docker compose --profile postgres up -d`.

## Passwords

Set exactly one of these:

- `SEI_PASSWORD`: the simplest, and the least safe. Anyone who can inspect the process or the
  container can read environment variables.
- `SEI_PASSWORD_FILE`: the path of a file that holds the password, such as a Docker secret.
- `SEI_PASSWORD_CMD`: a command that prints the password, for example `pass show sei/monitor`.
  sei-monitor runs it at every login, without a shell, with a 30-second limit, and without passing
  on its own secret variables. Tools such as `secret-tool` need a D-Bus session, which a container
  doesn't have, so this option fits best when sei-monitor runs outside Docker.

Every secret variable accepts the same `_FILE` form: `SEI_PASSWORD`, `DATABASE_URL`, `FEED_TOKEN`,
`API_TOKEN`, `SMTP_PASSWORD`, `TELEGRAM_TOKEN`, `NTFY_TOKEN` and `WEBHOOK_SECRET`.

A Docker secret for the password looks like this in `compose.yaml`:

```yaml
services:
  sei-monitor:
    environment:
      SEI_PASSWORD_FILE: /run/secrets/sei_password
    secrets:
      - sei_password

secrets:
  sei_password:
    file: ./sei_password.txt
```

The image runs as user 65532, so that user must be able to read the file.

The session token SEI hands out after the login is a credential too. sei-monitor keeps it only in
memory, never in the database, a file, a log or a message.

## Configuration

`.env.example` documents every variable. In short:

| Variable | Default | What it does |
|---|---|---|
| `SEI_URL` | | SEI address, without `/sei` at the end. |
| `SEI_API_URL` | discovered | Address of the `mod-wssei` API, only if the discovery fails. |
| `SEI_USERNAME` | | Your SEI login. |
| `SEI_PASSWORD`, `_FILE`, `_CMD` | | The password; set exactly one. |
| `SEI_ORG` | | Agency acronym or id, as `orgs` lists them. |
| `SEI_UNITS` | your current unit | Unit acronyms separated by commas, or `all`. |
| `SEI_TIMEZONE` | `America/Sao_Paulo` | Time zone of the dates SEI shows. |
| `POLL_INTERVAL` | `15m` | Time between checks, at least `5m`. |
| `SEI_CONCURRENCY` | `3` | Simultaneous calls to SEI, from 1 to 10. |
| `DATABASE_URL` | `sqlite:///data/sei-monitor.db` | `sqlite://<path>` or `postgres://...`. |
| `HTTP_ADDRESS` | `:8080` | Address of the HTTP server; empty turns it off. |
| `PUBLIC_URL` | | Public address behind nginx, used only for the feed's link to itself. |
| `FEED_TOKEN` | | Turns on the RSS feed. At least 32 characters. |
| `API_TOKEN` | | Turns on the JSON API. At least 32 characters, different from `FEED_TOKEN`. |
| `NOTIFICATION_DETAIL` | `minimal` | Detail of the feed and the messages: `minimal` or `full`. |
| `API_DETAIL` | `NOTIFICATION_DETAIL` | Detail of the API: `minimal` or `full`. |
| `RETURN_WARNING_DAYS` | `2` | How early to warn about a scheduled return. |
| `LOG_LEVEL` | `info` | `debug`, `info`, `warn` or `error`. |
| `SMTP_*`, `TELEGRAM_*`, `NTFY_*`, `WEBHOOK_*`, `WHATSAPP_RECIPIENTS` | | The channels, below. |

## Reading the events

### RSS feed

With `FEED_TOKEN` set, the feed lives at `/feed/<FEED_TOKEN>.xml` and holds the latest 100 events.
The token is the feed's only protection, so treat the address like a password. A wrong token gets
the same 404 as any unknown page, and sei-monitor's own access log masks the token. If you set
`PUBLIC_URL`, the feed links to itself, which puts its full address, token included, inside the
feed.

### JSON API

With `API_TOKEN` set, these routes answer to `Authorization: Bearer <API_TOKEN>`:

| Route | What it returns |
|---|---|
| `GET /status` | The agency, the watched units, the SEI versions, the latest check, the next one and the deliveries per channel. |
| `GET /processes` | The processes in your units. Filters: `unit`, `assigned_to_me`, `return_scheduled`, `return_overdue`, `include_left`. |
| `GET /processes/{procedure_id}` | One process, the documents sei-monitor has seen in it and its latest 50 events. |
| `GET /events` | The events, newest first. Filters: `kind` (repeatable), `procedure_id`, `since`, `until`. |
| `GET /deliveries` | The deliveries per channel. Filters: `status`, `channel`. |
| `POST /cycle` | Starts a check now. |
| `GET /openapi.json` | The OpenAPI document. It needs no token. |

```sh
curl -H "Authorization: Bearer $API_TOKEN" https://example.org/api/v1/status
curl -H "Authorization: Bearer $API_TOKEN" "https://example.org/api/v1/events?kind=document_added&limit=20"
```

Lists come as `{"items": [...], "next_cursor": "..."}`. Pass `next_cursor` back as `cursor` to read
the next page; it is `null` on the last one. `limit` goes from 1 to 200 and defaults to 50. Errors
use the `application/problem+json` format. `POST /cycle` answers 202 when the check starts, 409
while a check is running, while monitoring is paused or before sei-monitor first reaches SEI, and
429 with `Retry-After` when the last check started less than 5 minutes ago.

[`docs/openapi.json`](docs/openapi.json) describes every route and field.

### Detail level

At the `minimal` level, events carry the event type, the process number, the unit and the date.
`full` adds the process type and description, the activity text, the document name and who did
what. Choose it only if you are fine with that content passing through the channels you use.

Your own SEI login, which is often your CPF, never shows up. Texts say "you", the API answers
`assigned_to_me` and `activity_by_me`, and `/status` never returns the username.

## Notifications

Every 30 seconds sei-monitor sends each channel one message with everything new for it. The
message lists up to ten events and ends with "and N more" when there are others. When a channel
fails, sei-monitor tries again after 1 minute, 5 minutes, 30 minutes, 2 hours and 6 hours, and
then gives up on those events for that channel. `/status` counts the deliveries per channel, and
`/deliveries` shows why the last attempt failed. Each event goes to the channels that were on when
sei-monitor recorded it.

### Email

Set `SMTP_HOST`, `SMTP_FROM` and `SMTP_TO`, plus `SMTP_USERNAME` and `SMTP_PASSWORD` if the server
asks for a login. `SMTP_TLS` picks the encryption:

- `starttls`, the default, on port 587. The server must offer STARTTLS, or the email isn't sent.
- `tls` on port 465.
- `none` on port 25, which refuses a password.

The subject reads like `[sei-monitor] 3 updates on your processes`, and the body is plain text.

### Telegram

Create a bot with @BotFather and put its token in `TELEGRAM_TOKEN`. Send the bot any message, or add
it to a group, then open `https://api.telegram.org/bot<TELEGRAM_TOKEN>/getUpdates` and copy the
`chat.id` you find there into `TELEGRAM_CHAT_ID`. Group ids start with `-100`. A public channel
also works as `@channel_name`.

### ntfy

Set `NTFY_URL` to the server and topic, such as `https://ntfy.sh/<topic>`. On the public ntfy.sh,
anyone who knows a topic can read it, so pick a long random one (the output of `generate-token`
works) or use a server with access control and `NTFY_TOKEN`. Login failures and overdue returns
arrive with high priority.

### Webhook

sei-monitor posts the events as JSON to `WEBHOOK_URL`:

```json
{
  "events": [
    {
      "id": 42,
      "kind": "document_added",
      "procedure_id": 9001,
      "process_number": "00000.000001/2026-00",
      "unit": "SEDEMO/NUPROC",
      "title": "New document on process 00000.000001/2026-00",
      "occurred_at": "2026-11-04T17:13:05Z",
      "detected_at": "2026-11-04T17:15:00Z",
      "detail": { "document_protocol": "0012345" }
    }
  ]
}
```

Each event has the fields of the `Event` schema in `docs/openapi.json`, at the level set by
`NOTIFICATION_DETAIL`. The message from `send-test` is `{"test": true, "events": []}`. Any 2xx
answer counts as delivered, and sei-monitor doesn't follow redirects.

The `X-Sei-Monitor-Signature` header carries `sha256=` followed by the hex HMAC-SHA256 of the raw
body, keyed with `WEBHOOK_SECRET`. Check it before trusting a request. In Python:

```python
import hashlib
import hmac

def is_from_sei_monitor(secret: bytes, body: bytes, signature: str) -> bool:
    expected = "sha256=" + hmac.new(secret, body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, signature)
```

### WhatsApp

The WhatsApp channel uses an unofficial client library (whatsmeow), not an API offered by
WhatsApp. WhatsApp may block a number it sees doing this, so pair a spare number, never your
personal one.

1. Set `WHATSAPP_RECIPIENTS` to the phone numbers that get the messages, with country and area
   code (`5595991234567`), or to group ids, which end in `@g.us`.
2. Pair the number that sends the messages:
   `docker compose run --rm sei-monitor whatsapp-pair 5595991234567`. It prints a code. On the
   phone with that number, open WhatsApp > Linked devices > Link a device, tap "Link with phone
   number instead" and type the code.
3. Restart sei-monitor.

The WhatsApp session lives in sei-monitor's database, so protect the database like a password.
Until the number is paired, the WhatsApp messages wait in the queue. To pair another number, unlink
sei-monitor under Linked devices on the paired phone, start sei-monitor once so it drops the old
session, and run `whatsapp-pair` again.

## Behind nginx

sei-monitor speaks plain HTTP and expects a reverse proxy in front of it for HTTPS. The compose file
publishes it only on `127.0.0.1:8080`. Inside the `server` block of your site:

```nginx
location /api/v1/feed/ {
    access_log off;
    proxy_pass http://127.0.0.1:8080/feed/;
}

location /api/v1/ {
    proxy_pass http://127.0.0.1:8080/;
}
```

The first block keeps the feed token out of nginx's logs. sei-monitor doesn't need to know the
prefix: it never builds absolute addresses and ignores the `X-Forwarded-*` headers. The one
exception is the feed's link to itself, which comes from `PUBLIC_URL`.

## Health check

`GET /healthz` answers `{"ok": true}` with status 200 when the latest check finished without an
error less than three intervals ago, and `{"ok": false}` with 503 otherwise. Right after the start,
it answers 200 for one interval while the first check runs. The Docker image uses it for its
`HEALTHCHECK`.

## When SEI refuses the login

sei-monitor doesn't try again, because repeated failures can lock the account. It pauses, records
an `account_login_failed` event, delivers it through the channels and keeps serving the feed and
the API. Fix the configuration and restart it. Network errors and SEI outages are different: those
are retried after 30 seconds, then a minute, doubling up to the check interval.

## Commands

| Command | What it does |
|---|---|
| `serve` | Watches SEI, sends the notifications and serves the feed and the API. It runs when you give no command. |
| `test-login` | Logs in and shows the SEI and module versions and your units. |
| `orgs` | Lists the acronym and id of every agency on the SEI login page. It needs only `SEI_URL`. |
| `diagnose` | Checks how the SEI API behaves and prints only counts and types, so its output is safe to share. |
| `send-test` | Sends a test message through every channel that is on and says which one failed. |
| `whatsapp-pair` | Pairs the WhatsApp number that sends the messages. |
| `generate-token` | Prints a random token for `FEED_TOKEN` or `API_TOKEN`. |

## Known limitations

- Agencies without `mod-wssei` aren't supported. sei-monitor doesn't read SEI's web pages, apart
  from the login page that lists the agencies.
- Special follow-ups (acompanhamentos especiais) aren't monitored, because the module's route for
  them fails on at least one SEI instance.
- Suspended processes (sobrestados) disappear from the module's process list, so sei-monitor
  reports them as having left your units, and as new when they come back.
- Accounts with two-factor authentication are untested.
- One SEI account per sei-monitor instance.
- Messages show the process number without a link, since SEI's internal links depend on the
  session.
- A captcha or Cloudflare challenge in front of SEI stops sei-monitor.

## Security

- Secrets (the password, the tokens, the SMTP password, the webhook secret) print as `[redacted]`
  everywhere, and none of them goes into the database.
- On Linux, sei-monitor turns off core dumps and blocks other processes from reading its memory
  through debugging interfaces. It has no profiling endpoint.
- Calls to SEI and to the email, Telegram, ntfy and webhook channels verify TLS and time out after
  30 seconds. Logs never include request headers, and errors never include addresses that may
  carry a token.
- The feed and the API compare their tokens in constant time. The API sends `no-store`, has no
  CORS and never returns the username.
- Webhook bodies are signed with HMAC-SHA256.
- The image runs as an unprivileged user, and `/data` is the only place it writes. The compose file
  also makes the rest of the filesystem read-only and drops every Linux capability. Files
  sei-monitor creates, such as the database, which also holds the WhatsApp session, can be read
  only by its own user.
- To keep the load on SEI low, checks run at least 5 minutes apart, with a cap on simultaneous
  calls and some random spread between checks.

## Development

- `make check` runs gofmt, go vet, golangci-lint and the tests with the race detector.
- The storage tests run on SQLite and on Postgres. Point `TEST_DATABASE_URL` at a Postgres server,
  or install `initdb` and `postgres` and the tests start a throwaway one. With `CI` set, a missing
  Postgres fails the tests instead of skipping them.
- `make openapi` regenerates `docs/openapi.json`, and a test fails while the file lags behind the
  code.
- `git config core.hooksPath .githooks` turns on the hooks: Conventional Commits messages, and
  gofmt plus the tests before each commit.
- No test talks to a real SEI. `internal/sei/seitest` plays its part.

## License

The license hasn't been chosen yet.
