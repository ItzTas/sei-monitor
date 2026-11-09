package main

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"time"

	"golang.org/x/sync/errgroup"

	"github.com/ItzTas/sei-monitor/internal/api"
	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/feed"
	"github.com/ItzTas/sei-monitor/internal/httpserver"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/notify"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/store"
	"github.com/ItzTas/sei-monitor/internal/ui"
	"github.com/ItzTas/sei-monitor/internal/whatsapp"
)

type application struct {
	cfg        config.Config
	log        *slog.Logger
	now        func() time.Time
	store      *store.Store
	monitor    *monitor.Monitor
	dispatcher *notify.Dispatcher
	whatsapp   *whatsapp.Session
	server     *httpserver.Server
}

func assemble(ctx context.Context, cfg config.Config, logger *slog.Logger, now func() time.Time) (*application, error) {
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
		RetentionDays:     cfg.RetentionDays,
		Location:          cfg.SEI.Location,
		Channels:          cfg.Channels.Enabled(),
	}, st, logger, monitor.WithClock(now, time.After))
	app := &application{cfg: cfg, log: logger, now: now, store: st, monitor: mon}
	channels := notify.Channels(cfg.Channels, userAgent())
	if cfg.Channels.WhatsApp != nil {
		session, err := whatsapp.Open(ctx, st.PassthroughDB(), st.Dialect(), logger)
		if err != nil {
			return nil, errors.Join(err, st.Close())
		}
		app.whatsapp = session
		channels = withWhatsApp(channels, session, cfg.Channels.WhatsApp.Recipients, logger)
	}
	app.dispatcher = notify.NewDispatcher(st, channels, app.formatter(cfg.FullDetail), logger, notify.WithClock(now, time.After))
	if cfg.HTTP.Enabled() {
		app.server = httpserver.New(cfg.HTTP.Address, app.routes(), logger)
	}
	return app, nil
}

func (a *application) routes() httpserver.Routes {
	routes := httpserver.Routes{Health: httpserver.Health(a.lastRun, a.cfg.PollInterval, a.now(), a.now, a.log)}
	if !a.cfg.HTTP.FeedToken.IsZero() {
		routes.Feed = feed.NewHandler(a.store, a.cfg.HTTP.FeedToken, a.feedChannel, a.formatter(a.cfg.FullDetail), a.now, a.log)
	}
	if !a.cfg.HTTP.APIToken.IsZero() {
		deps := api.Deps{Store: a.store, Monitor: a.monitor, Token: a.cfg.HTTP.APIToken, FullDetail: a.cfg.APIFullDetail, Location: a.cfg.SEI.Location, Log: a.log}
		routes.API = func(mux *http.ServeMux) { api.Register(mux, deps) }
		pages := ui.Deps{Store: a.store, Monitor: a.monitor, Token: a.cfg.HTTP.APIToken, FullDetail: a.cfg.APIFullDetail, Location: a.cfg.SEI.Location, Now: a.now, Log: a.log}
		routes.UI = func(mux *http.ServeMux) { ui.Register(mux, pages) }
	}
	return routes
}

func (a *application) formatter(full bool) func() event.Formatter {
	return func() event.Formatter {
		return event.Formatter{Location: a.cfg.SEI.Location, Full: full, Self: a.monitor.State().Self.Acronym}
	}
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
	if a.cfg.HTTP.PublicURL != "" {
		channel.Self = a.cfg.HTTP.PublicURL + "/feed/" + a.cfg.HTTP.FeedToken.Reveal() + ".xml"
	}
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
	return a.store.Close()
}
