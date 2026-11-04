package main

import (
	"bytes"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io"
	"log/slog"
	"maps"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/notify"
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

		type hook struct {
			signature string
			body      []byte
		}
		hooks := make(chan hook, 4)
		webhook := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			body, _ := io.ReadAll(r.Body)
			hooks <- hook{signature: r.Header.Get(notify.SignatureHeader), body: body}
			w.WriteHeader(http.StatusNoContent)
		}))
		t.Cleanup(webhook.Close)
		feedToken := "feed-token-0123456789abcdefghijklmnopqrstuv"
		apiToken := "api-token-0123456789abcdefghijklmnopqrstuvw"
		env := seiEnv(server, map[string]string{
			"DATABASE_URL":   databaseURL,
			"HTTP_ADDRESS":   "127.0.0.1:8080",
			"FEED_TOKEN":     feedToken,
			"API_TOKEN":      apiToken,
			"PUBLIC_URL":     "https://monitor.example.org/api/v1",
			"WEBHOOK_URL":    webhook.URL + "/sei",
			"WEBHOOK_SECRET": "0123456789abcdef",
		})
		cfg, err := config.Load(envSource(env))
		if err != nil {
			t.Fatal(err)
		}
		now := start
		logs := &bytes.Buffer{}
		logger := slog.New(slog.NewTextHandler(logs, &slog.HandlerOptions{Level: slog.LevelDebug}))
		application, err := assemble(t.Context(), cfg, logger, func() time.Time { return now })
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

		handler := application.server.Handler()
		get := func(target, token string) *httptest.ResponseRecorder {
			request := httptest.NewRequestWithContext(ctx, http.MethodGet, target, nil)
			if token != "" {
				request.Header.Set("Authorization", "Bearer "+token)
			}
			response := httptest.NewRecorder()
			handler.ServeHTTP(response, request)
			return response
		}
		feed := get("/feed/"+feedToken+".xml", "")
		body := feed.Body.String()
		for _, part := range []string{"<title>sei-monitor: SEDEMO</title>", "New activity on process " + seitest.ProcessNumber(1), "Process " + seitest.ProcessNumber(2) + " left SEDEMO/NUPROC"} {
			if feed.Code != http.StatusOK || !strings.Contains(body, part) {
				t.Errorf("feed (%d) does not contain %q:\n%s", feed.Code, part, body)
			}
		}
		if self := `<atom:link href="https://monitor.example.org/api/v1/feed/` + feedToken + `.xml" rel="self" type="application/rss+xml"></atom:link>`; !strings.Contains(body, self) {
			t.Errorf("feed misses the self link %s:\n%s", self, body)
		}
		health := get("/healthz", "")
		if health.Code != http.StatusOK || strings.TrimSpace(health.Body.String()) != `{"ok":true}` {
			t.Errorf("health = %d %q", health.Code, health.Body.String())
		}

		application.dispatcher.RunOnce(ctx)
		delivered := <-hooks
		mac := hmac.New(sha256.New, []byte("0123456789abcdef"))
		mac.Write(delivered.body)
		var payload struct {
			Events []event.View `json:"events"`
		}
		if err := json.Unmarshal(delivered.body, &payload); err != nil {
			t.Fatal(err)
		}
		var deliveredKinds []event.Kind
		for _, view := range payload.Events {
			deliveredKinds = append(deliveredKinds, view.Kind)
		}
		slices.Sort(deliveredKinds)
		if delivered.signature != "sha256="+hex.EncodeToString(mac.Sum(nil)) || !slices.Equal(deliveredKinds, []event.Kind{event.ActivityAdded, event.ProcessLeft}) {
			t.Errorf("webhook got %s with signature %s", delivered.body, delivered.signature)
		}

		status := get("/status", apiToken)
		var state struct {
			OrgAcronym string   `json:"org_acronym"`
			Units      []string `json:"units"`
			LastCycle  struct {
				EventsCreated int `json:"events_created"`
			} `json:"last_cycle"`
			Deliveries map[string]map[string]int `json:"deliveries"`
		}
		if err := json.Unmarshal(status.Body.Bytes(), &state); err != nil || status.Code != http.StatusOK {
			t.Fatalf("status = %d %s (%v)", status.Code, status.Body.String(), err)
		}
		sent := map[string]int{"pending": 0, "sent": 2, "failed": 0}
		if state.OrgAcronym != "SEDEMO" || !slices.Equal(state.Units, []string{"SEDEMO/NUPROC"}) || state.LastCycle.EventsCreated != 2 ||
			!maps.Equal(state.Deliveries["webhook"], sent) || strings.Contains(status.Body.String(), seitest.Username) {
			t.Errorf("status = %s", status.Body.String())
		}
		processes := get("/processes", apiToken)
		var list struct {
			Items []struct {
				ProcedureID int64 `json:"procedure_id"`
			} `json:"items"`
		}
		if err := json.Unmarshal(processes.Body.Bytes(), &list); err != nil || len(list.Items) != 1 || list.Items[0].ProcedureID != 9001 {
			t.Errorf("processes = %d %s (%v)", processes.Code, processes.Body.String(), err)
		}

		if !strings.Contains(logs.String(), "path=/feed/[redacted]") || !strings.Contains(logs.String(), "path=/status") {
			t.Errorf("the access log misses the requests:\n%s", logs.String())
		}
		for _, secret := range []string{feedToken, apiToken, seitest.Password, "Authorization", "Bearer"} {
			if strings.Contains(logs.String(), secret) {
				t.Errorf("the logs contain %q:\n%s", secret, logs.String())
			}
		}
	})
}

func TestAPIRoutesStayOffWithoutAnAPIToken(t *testing.T) {
	env := seiEnv(seitest.New(t), map[string]string{
		"DATABASE_URL": storetest.SQLiteURL(t),
		"HTTP_ADDRESS": "127.0.0.1:8080",
		"FEED_TOKEN":   "feed-token-0123456789abcdefghijklmnopqrstuv",
	})
	cfg, err := config.Load(envSource(env))
	if err != nil {
		t.Fatal(err)
	}
	application, err := assemble(t.Context(), cfg, slog.New(slog.DiscardHandler), time.Now)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = application.Close() })

	for _, target := range []string{"/status", "/processes", "/openapi.json"} {
		request := httptest.NewRequestWithContext(t.Context(), http.MethodGet, target, nil)
		request.Header.Set("Authorization", "Bearer api-token-0123456789abcdefghijklmnopqrstuvw")
		response := httptest.NewRecorder()
		application.server.Handler().ServeHTTP(response, request)
		if response.Code != http.StatusNotFound {
			t.Errorf("GET %s without API_TOKEN = %d", target, response.Code)
		}
	}
}
