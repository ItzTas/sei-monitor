package main

import (
	"io"
	"log/slog"
	"slices"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
	"github.com/ItzTas/sei-monitor/internal/store"
	"github.com/ItzTas/sei-monitor/internal/store/storetest"
)

func TestTwoCyclesThroughTheWholeApp(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		server := seitest.New(t)
		start := time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)
		server.SetProcesses(seitest.UnitNUPROC,
			seitest.Process{ID: 9001, Number: seitest.ProcessNumber(1), Type: "Contrato"},
			seitest.Process{ID: 9002, Number: seitest.ProcessNumber(2), Type: "Diárias"},
		)
		server.AddActivities(9001, seitest.Activity{ID: 50001, Unit: "SEDEMO/NUPROC", Text: "Processo gerado", At: start.Add(-time.Hour)})
		server.AddActivities(9002, seitest.Activity{ID: 60001, Unit: "SEDEMO/NUPROC", Text: "Processo gerado", At: start.Add(-time.Hour)})

		env := seiEnv(server, map[string]string{
			"DATABASE_URL":   databaseURL,
			"HTTP_ADDRESS":   "",
			"WEBHOOK_URL":    "https://hooks.example.org/sei",
			"WEBHOOK_SECRET": "0123456789abcdef",
		})
		cfg, err := config.Load(config.Source{Lookup: func(name string) (string, bool) { value, ok := env[name]; return value, ok }})
		if err != nil {
			t.Fatal(err)
		}
		now := start
		clock := monitor.WithClock(func() time.Time { return now }, time.After)
		application, err := assemble(t.Context(), cfg, slog.New(slog.NewTextHandler(io.Discard, nil)), clock)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() {
			if err := application.Close(); err != nil {
				t.Error(err)
			}
		})
		ctx := t.Context()
		if err := application.monitor.Prepare(ctx); err != nil {
			t.Fatal(err)
		}

		if _, err := application.monitor.RunOnce(ctx, store.TriggerScheduled); err != nil {
			t.Fatal(err)
		}
		now = start.Add(15 * time.Minute)
		server.RemoveProcess(seitest.UnitNUPROC, 9002)
		server.AddActivities(9001, seitest.Activity{ID: 50002, Unit: "SEDEMO/NUPROC", Text: "Processo recebido na unidade", At: now})
		if _, err := application.monitor.RunOnce(ctx, store.TriggerScheduled); err != nil {
			t.Fatal(err)
		}

		events, err := application.store.RecentEvents(ctx, 10)
		if err != nil {
			t.Fatal(err)
		}
		var kinds []event.Kind
		for _, ev := range events {
			kinds = append(kinds, ev.Kind)
		}
		if !slices.Equal(kinds, []event.Kind{event.ActivityAdded, event.ProcessLeft}) {
			t.Errorf("events, newest first = %v", kinds)
		}
	})
}
