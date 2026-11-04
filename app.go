package main

import (
	"context"
	"log/slog"
	"time"

	"golang.org/x/sync/errgroup"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/feed"
	"github.com/ItzTas/sei-monitor/internal/httpserver"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/store"
)

type application struct {
	cfg     config.Config
	log     *slog.Logger
	store   *store.Store
	monitor *monitor.Monitor
	server  *httpserver.Server
}

func assemble(ctx context.Context, cfg config.Config, logger *slog.Logger, options ...monitor.Option) (*application, error) {
	st, err := store.Open(ctx, cfg.DatabaseURL.Reveal())
	if err != nil {
		return nil, err
	}
	target := sei.Target{SEIURL: cfg.SEI.URL, APIURL: cfg.SEI.APIURL, Org: cfg.SEI.Org, Username: cfg.SEI.Username, Password: cfg.SEI.Password}
	mon := monitor.New(monitor.Config{
		Target:            target,
		Options:           seiOptions(cfg.SEI.Location),
		Units:             cfg.SEI.Units,
		AllUnits:          cfg.SEI.AllUnits,
		PollInterval:      cfg.PollInterval,
		Concurrency:       cfg.SEI.Concurrency,
		ReturnWarningDays: cfg.ReturnWarningDays,
		Location:          cfg.SEI.Location,
		Channels:          cfg.Channels.Enabled(),
	}, st, logger, options...)
	app := &application{cfg: cfg, log: logger, store: st, monitor: mon}
	if cfg.HTTP.Enabled() {
		app.server = httpserver.New(cfg.HTTP.Address, app.routes(), logger)
	}
	return app, nil
}

func (a *application) routes() httpserver.Routes {
	routes := httpserver.Routes{Health: httpserver.Health(a.lastRun, a.cfg.PollInterval, time.Now(), time.Now, a.log)}
	if !a.cfg.HTTP.FeedToken.IsZero() {
		formatter := event.Formatter{Location: a.cfg.SEI.Location, Full: a.cfg.FullDetail}
		routes.Feed = feed.NewHandler(a.store, a.cfg.HTTP.FeedToken, a.feedChannel, formatter, time.Now, a.log)
	}
	return routes
}

func (a *application) lastRun(ctx context.Context) (store.CycleRun, bool, error) {
	accountID := a.monitor.State().AccountID
	if accountID == 0 {
		return store.CycleRun{}, false, nil
	}
	return a.store.LastCycleRun(ctx, accountID)
}

func (a *application) feedChannel(ctx context.Context) feed.Channel {
	channel := feed.Channel{Title: "sei-monitor", Link: a.cfg.SEI.URL}
	accountID := a.monitor.State().AccountID
	if accountID == 0 {
		return channel
	}
	account, err := a.store.Account(ctx, accountID)
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
	if a.server != nil {
		g.Go(func() error { return a.server.Run(gctx) })
	}
	return g.Wait()
}

func (a *application) Close() error {
	return a.store.Close()
}
