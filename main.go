package main

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/joho/godotenv"
	"golang.org/x/sync/errgroup"

	"github.com/ItzTas/sei-monitor/internal/cli"
	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/feed"
	"github.com/ItzTas/sei-monitor/internal/modules/events"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/notify"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/server"
	"github.com/ItzTas/sei-monitor/internal/ui"
	"github.com/ItzTas/sei-monitor/internal/whatsapp"
)

type application struct {
	cfg        core.Config
	log        *slog.Logger
	now        func() time.Time
	database   *core.Database
	repo       *monitor.Postgres
	monitor    *monitor.Monitor
	dispatcher *notify.Dispatcher
	whatsapp   *whatsapp.Session
	server     *server.Server
}

func main() {
	if err := cli.Harden(); err != nil {
		_, _ = fmt.Fprintf(os.Stderr, "warning: could not turn off core dumps and process inspection: %v\n", err)
	}
	if err := godotenv.Load(); err != nil && !errors.Is(err, fs.ErrNotExist) {
		_, _ = fmt.Fprintf(os.Stderr, "error: read .env: %v\n", err)
		os.Exit(1)
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	terminal := cli.Terminal{Stdin: os.Stdin, Stdout: os.Stdout, Stderr: os.Stderr}
	code := cli.Run(ctx, os.Args[1:], terminal, core.Environment(), serve)
	stop()
	os.Exit(code)
}

func serve(ctx context.Context, cfg core.Config, logger *slog.Logger) (err error) {
	app, err := assemble(ctx, cfg, logger, time.Now)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, app.Close()) }()
	logger.Info("sei-monitor started",
		"version", core.Version(),
		"interval", cfg.PollInterval,
		"channels", strings.Join(cfg.Channels.Enabled(), ","),
	)
	return app.Run(ctx)
}

func assemble(ctx context.Context, cfg core.Config, logger *slog.Logger, now func() time.Time) (*application, error) {
	database, err := core.OpenDatabase(ctx, cfg.DatabaseURL.Reveal())
	if err != nil {
		return nil, err
	}
	repo := monitor.NewPostgres(database.Pool)
	target := sei.Target{SEIURL: cfg.SEI.URL, APIURL: cfg.SEI.APIURL, Org: cfg.SEI.Org, Username: cfg.SEI.Username, Password: cfg.SEI.Password}
	mon := monitor.New(monitor.Config{
		Target:            target,
		Options:           sei.Options{UserAgent: core.UserAgent(), Location: cfg.SEI.Location},
		Units:             cfg.SEI.Units,
		AllUnits:          cfg.SEI.AllUnits,
		PollInterval:      cfg.PollInterval,
		Concurrency:       cfg.SEI.Concurrency,
		ReturnWarningDays: cfg.ReturnWarningDays,
		RetentionDays:     cfg.RetentionDays,
		Location:          cfg.SEI.Location,
		Channels:          cfg.Channels.Enabled(),
	}, repo, logger, monitor.WithClock(now, time.After))
	app := &application{cfg: cfg, log: logger, now: now, database: database, repo: repo, monitor: mon}
	channels := notify.Channels(cfg.Channels, core.UserAgent())
	if cfg.Channels.WhatsApp != nil {
		session, err := whatsapp.Open(ctx, database.SQL, logger)
		if err != nil {
			return nil, errors.Join(err, database.Close())
		}
		app.whatsapp = session
		channels = withWhatsApp(channels, session, cfg.Channels.WhatsApp.Recipients, logger)
	}
	outbox := notify.NewPostgres(database.Pool)
	app.dispatcher = notify.NewDispatcher(outbox, channels, app.formatter(cfg.FullDetail), logger, notify.WithClock(now, time.After))
	if cfg.HTTP.Enabled() {
		app.server = server.New(cfg.HTTP.Address, app.routes(), logger)
	}
	return app, nil
}

func withWhatsApp(channels []notify.Channel, session *whatsapp.Session, recipients []string, log *slog.Logger) []notify.Channel {
	if session.Paired() {
		return append(channels, whatsapp.NewChannel(session, recipients))
	}
	log.Warn("WhatsApp is set up but not paired; its notifications wait until you run sei-monitor whatsapp-pair and restart")
	return channels
}

func (a *application) routes() server.Routes {
	routes := server.Routes{Health: server.Health(a.lastRun, a.cfg.PollInterval, a.now(), a.now, a.log)}
	if !a.cfg.HTTP.FeedToken.IsZero() {
		routes.Feed = feed.NewHandler(feed.NewPostgres(a.database.Pool), a.cfg.HTTP.FeedToken, a.feedChannel, a.formatter(a.cfg.FullDetail), a.now, a.log)
	}
	if a.cfg.HTTP.APIToken.IsZero() {
		return routes
	}
	deps := server.APIDeps{Pool: a.database.Pool, Monitor: a.monitor, Token: a.cfg.HTTP.APIToken, FullDetail: a.cfg.APIFullDetail, Location: a.cfg.SEI.Location, Log: a.log}
	routes.API = func(mux *http.ServeMux) { server.RegisterAPI(mux, deps) }
	pages := ui.Deps{Events: events.NewPostgres(a.database.Pool), Monitor: a.monitor, Token: a.cfg.HTTP.APIToken, FullDetail: a.cfg.APIFullDetail, Location: a.cfg.SEI.Location, Now: a.now, Log: a.log}
	routes.UI = func(mux *http.ServeMux) { ui.Register(mux, pages) }
	return routes
}

func (a *application) formatter(full bool) func() event.Formatter {
	return func() event.Formatter {
		return event.Formatter{Location: a.cfg.SEI.Location, Full: full, Self: a.monitor.State().Self.Acronym}
	}
}

func (a *application) lastRun(ctx context.Context) (core.CycleRun, bool, error) {
	accountID := a.monitor.State().AccountID
	if accountID == 0 {
		return core.CycleRun{}, false, nil
	}
	return a.repo.LastCycleRun(ctx, accountID)
}

func (a *application) feedChannel(ctx context.Context) feed.Channel {
	channel := feed.Channel{Title: "sei-monitor", Link: a.cfg.SEI.URL}
	if a.cfg.HTTP.PublicURL != "" {
		channel.Self = a.cfg.HTTP.PublicURL + "/feed/" + a.cfg.HTTP.FeedToken.Reveal() + ".xml"
	}
	accountID := a.monitor.State().AccountID
	if accountID == 0 {
		return channel
	}
	account, err := a.repo.Account(ctx, accountID)
	if err != nil {
		a.log.Warn("could not read the agency acronym for the feed title", "error", err)
		return channel
	}
	if account.Info.OrgAcronym != "" {
		channel.Title += ": " + account.Info.OrgAcronym
	}
	return channel
}

func (a *application) Run(ctx context.Context) error {
	g, gctx := errgroup.WithContext(ctx)
	g.Go(func() error { return a.monitor.Run(gctx) })
	g.Go(func() error { return a.dispatcher.Run(gctx) })
	if a.server != nil {
		g.Go(func() error { return a.server.Run(gctx) })
	}
	return g.Wait()
}

func (a *application) Close() error {
	if a.whatsapp != nil {
		a.whatsapp.Close()
	}
	return a.database.Close()
}
