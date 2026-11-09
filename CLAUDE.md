# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# sei-monitor

Self-hosted watcher for SEI processes, in Go. It logs in through SEI's REST module (`mod-wssei`),
checks the processes of the user's units every few minutes, turns what changed into events and
delivers them by RSS, a JSON API, a small web UI and notification channels (email, Telegram, ntfy,
webhook, AMQP, WhatsApp). The root `main.go` only assembles the pieces; the code lives in
`internal/`.

## Read the `project-structure` skill first

Before creating or moving a package, a module, a migration, a query or any config/scaffolding file,
read `.claude/skills/project-structure/SKILL.md`. It is the source of truth for where things go and
how the layers depend on each other.

## Code

- **No comments in the code.** No line, block or doc comments. Names explain what the code does.
  Compiler directives (`//go:embed`, `//go:build`) are not comments and stay. Generated code in
  `internal/db` keeps the header sqlc writes.
- **English everywhere**: identifiers, packages, file and directory names, SQL tables/columns,
  migrations, logs, error messages, CLI output, API fields, docs.
- Never hand-edit `internal/db`: edit `sql/queries` or `sql/schema` and run `make generate`.
- A schema change is a new numbered migration in `sql/schema`; never edit one that exists.

## Commands

```bash
make check            # gofmt, go vet, golangci-lint, go test -race
make test             # go test -race ./...
make generate         # go tool sqlc generate (sql/ -> internal/db)
make dev              # go tool air, hot reload with .env
make local CMD=serve  # go run . <command> with .env
make openapi          # regenerate docs/openapi.json (a test fails while it lags)
make up / make down   # docker compose (app + postgres)
```

- `air`, `sqlc` and `goose` are Go tool dependencies (`tool` block in `go.mod`); always run them
  through `go tool`.
- `main.go` loads `.env` with godotenv (a missing file is ignored; variables already in the
  environment win).
- Tests that touch the database use `internal/core/coretest`: each package with such tests has a
  `database_test.go` whose `TestMain` calls `coretest.Run`. It starts a throwaway Postgres from the
  machine's `initdb`/`postgres` (or uses `TEST_DATABASE_URL`), migrates a template database once and
  `coretest.Open(t)` / `coretest.DatabaseURL(t)` hand each test a fresh copy. With `CI` set a
  missing Postgres fails instead of skipping. The AMQP tests need `TEST_AMQP_URL`.
- The root `integration_test.go` builds the whole app through the same `assemble` as `main.go`,
  against the fake SEI in `internal/sei/seitest`.
- CI (`.github/workflows/ci.yml`): gofmt, vet, a check that `internal/db` matches the queries,
  golangci-lint, and `go test -race` with Postgres and RabbitMQ services. `docker.yml` publishes the
  multi-arch image to `ghcr.io`.
- Git hooks: `git config core.hooksPath .githooks` (Conventional Commits; gofmt + tests).

## Configuration

Every variable is documented in `.env.example` and the README. `internal/core/config.go`
(`core.Load`) reads them through a `core.Source`, collects every problem and reports them together.
`DATABASE_URL` is required and must be a `postgres://` address. Secrets accept `NAME_FILE`, and the
SEI password also `SEI_PASSWORD_CMD`. `RETENTION_DAYS` must be at least `RETURN_WARNING_DAYS + 2`
(or 0): `return_due_soon` events are deduplicated only by the stored event, so pruning one while
its return is still near would send it again.

## Architecture

- `main.go`: `main` hardens the process (`cli.Harden`), loads `.env` and calls `cli.Run` with
  `serve`; `assemble` opens the database, builds the monitor, the dispatcher with its channels,
  WhatsApp and the HTTP server; `application.Run` runs them in an errgroup.
- `internal/cli`: the cobra commands (`serve`, `orgs`, `test-login`, `diagnose`, `send-test`,
  `whatsapp-pair`, `generate-token`, hidden `openapi`) and the process hardening.
- `internal/core`: configuration, `OpenDatabase` (migrates, then pgxpool plus a `database/sql`
  handle for whatsmeow), `InTx`, the version, and the shared records (`Account`, `Process`,
  `Document`, `CycleRun`, `Delivery`, `DueDelivery`) with their converters from sqlc rows.
- `internal/sei`: SEI client (discovery, login, listings); `seitest` is the fake server.
- `internal/monitor`: the cycle (scheduler, diff, events) and its `Repository`/`Postgres`, which
  owns every write of a cycle (`ApplyCycle` in one transaction), accounts, cycle runs and pruning.
- `internal/notify`: channels, the dispatcher and its outbox `Postgres` (due deliveries, attempts).
- `internal/event`: event kinds, payloads and formatting; no dependencies on other packages.
- `internal/modules/<name>`: one HTTP feature each (`status`, `processes`, `events`,
  `deliveries`, `cycle`), layered `types.go` / `repository.go` / `service.go` / `handler.go` /
  `routes.go`.
- `internal/server`: the HTTP server (timeouts, security headers, access log, `/healthz`) and
  `RegisterAPI`, which builds the huma API, bearer auth and calls each module's `Configure`.
- `internal/feed` (RSS), `internal/ui` (htmx pages over the `events` module), `internal/resp`
  (pagination, cursors, error mapping), `internal/whatsapp` (whatsmeow session and channel).
- The database is Postgres only. whatsmeow keeps its own `whatsmeow_*` tables in the same database
  and manages their schema; don't touch them.
