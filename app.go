package main

import (
	"context"
	"log/slog"

	"golang.org/x/sync/errgroup"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/store"
)

type application struct {
	cfg     config.Config
	log     *slog.Logger
	store   *store.Store
	monitor *monitor.Monitor
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
	return &application{cfg: cfg, log: logger, store: st, monitor: mon}, nil
}

func (a *application) Run(ctx context.Context) error {
	g, gctx := errgroup.WithContext(ctx)
	g.Go(func() error { return a.monitor.Run(gctx) })
	return g.Wait()
}

func (a *application) Close() error {
	return a.store.Close()
}
