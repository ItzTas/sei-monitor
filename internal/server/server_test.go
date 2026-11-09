package server

import (
	"bytes"
	"context"
	"errors"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
)

var startedAt = time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)

func discard() *slog.Logger {
	return slog.New(slog.DiscardHandler)
}

func get(t *testing.T, handler http.Handler, path string, headers map[string]string) *httptest.ResponseRecorder {
	t.Helper()
	request := httptest.NewRequestWithContext(t.Context(), http.MethodGet, path, nil)
	for name, value := range headers {
		request.Header.Set(name, value)
	}
	recorder := httptest.NewRecorder()
	handler.ServeHTTP(recorder, request)
	return recorder
}

func okHandler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { _, _ = io.WriteString(w, "ok") })
}

func TestHealth(t *testing.T) {
	interval := 15 * time.Minute
	tests := []struct {
		name  string
		run   core.CycleRun
		found bool
		err   error
		now   time.Time
		ok    bool
	}{
		{name: "no cycle yet, inside the grace period", now: startedAt.Add(5 * time.Minute), ok: true},
		{name: "no cycle after the grace period", now: startedAt.Add(16 * time.Minute)},
		{name: "recent good cycle", run: core.CycleRun{FinishedAt: startedAt.Add(20 * time.Minute)}, found: true, now: startedAt.Add(30 * time.Minute), ok: true},
		{name: "failed cycle", run: core.CycleRun{FinishedAt: startedAt.Add(20 * time.Minute), Error: "SEI answered with an internal error"}, found: true, now: startedAt.Add(21 * time.Minute)},
		{name: "good cycle older than three intervals", run: core.CycleRun{FinishedAt: startedAt.Add(20 * time.Minute)}, found: true, now: startedAt.Add(65 * time.Minute)},
		{name: "failed cycle from before the restart, inside the grace period", run: core.CycleRun{FinishedAt: startedAt.Add(-time.Minute), Error: "x"}, found: true, now: startedAt.Add(time.Minute), ok: true},
		{name: "cycle still running after the grace period", run: core.CycleRun{StartedAt: startedAt}, found: true, now: startedAt.Add(16 * time.Minute)},
		{name: "database error", err: errors.New("store: read the last cycle: database is locked"), now: startedAt.Add(time.Minute)},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			lastRun := func(context.Context) (core.CycleRun, bool, error) { return tt.run, tt.found, tt.err }
			handler := Health(lastRun, interval, startedAt, func() time.Time { return tt.now }, discard())

			response := get(t, handler, "/healthz", nil)

			wantStatus, wantBody := http.StatusServiceUnavailable, `{"ok":false}`
			if tt.ok {
				wantStatus, wantBody = http.StatusOK, `{"ok":true}`
			}
			if response.Code != wantStatus || strings.TrimSpace(response.Body.String()) != wantBody || response.Header().Get("Content-Type") != "application/json" {
				t.Errorf("response = %d %q %q, want %d %s", response.Code, response.Header().Get("Content-Type"), response.Body.String(), wantStatus, wantBody)
			}
		})
	}
}

func TestEveryResponseHasTheSecurityHeaders(t *testing.T) {
	handler := Handler(Routes{Health: okHandler(), Feed: okHandler()}, discard())

	for _, path := range []string{"/healthz", "/feed/x.xml", "/nothing-here"} {
		response := get(t, handler, path, nil)
		if response.Header().Get("Cache-Control") != "no-store" || response.Header().Get("X-Content-Type-Options") != "nosniff" {
			t.Errorf("%s: headers = %v", path, response.Header())
		}
	}
}

func TestFeedIsOnlyMountedWithItsHandler(t *testing.T) {
	handler := Handler(Routes{Health: okHandler()}, discard())

	if response := get(t, handler, "/feed/x.xml", nil); response.Code != http.StatusNotFound {
		t.Errorf("feed without a handler answered %d", response.Code)
	}
}

func TestAccessLogHidesTokensQueriesAndHeaders(t *testing.T) {
	var logs bytes.Buffer
	logger := slog.New(slog.NewTextHandler(&logs, &slog.HandlerOptions{Level: slog.LevelInfo}))
	handler := Handler(Routes{Health: okHandler(), Feed: okHandler()}, logger)
	headers := map[string]string{"Authorization": "Bearer api-token-value", "Cookie": "session=cookie-value"}

	get(t, handler, "/feed/feed-token-value.xml?key=query-value", headers)
	get(t, handler, "//feed/feed-token-value.xml", headers)
	get(t, handler, "/healthz", headers)

	output := logs.String()
	for _, secret := range []string{"feed-token-value", "query-value", "api-token-value", "cookie-value", "Bearer"} {
		if strings.Contains(output, secret) {
			t.Errorf("the access log shows %q:\n%s", secret, output)
		}
	}
	if !strings.Contains(output, "path=/feed/[redacted]") || !strings.Contains(output, "status=200") {
		t.Errorf("the access log misses the masked feed request:\n%s", output)
	}
	if strings.Contains(output, "/healthz") {
		t.Errorf("health checks are logged at info level:\n%s", output)
	}
}

func TestRequestBodiesAreLimited(t *testing.T) {
	routes := Routes{Health: okHandler(), API: func(mux *http.ServeMux) {
		mux.HandleFunc("POST /echo", func(w http.ResponseWriter, r *http.Request) {
			if _, err := io.ReadAll(r.Body); err != nil {
				http.Error(w, "too large", http.StatusRequestEntityTooLarge)
				return
			}
			w.WriteHeader(http.StatusNoContent)
		})
	}}
	handler := Handler(routes, discard())

	for size, want := range map[int]int{MaxBodyBytes: http.StatusNoContent, MaxBodyBytes + 1: http.StatusRequestEntityTooLarge} {
		request := httptest.NewRequestWithContext(t.Context(), http.MethodPost, "/echo", strings.NewReader(strings.Repeat("x", size)))
		recorder := httptest.NewRecorder()
		handler.ServeHTTP(recorder, request)
		if recorder.Code != want {
			t.Errorf("body of %d bytes: status %d, want %d", size, recorder.Code, want)
		}
	}
}

func TestServeStopsWhenTheContextEnds(t *testing.T) {
	var config net.ListenConfig
	listener, err := config.Listen(t.Context(), "tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	server := New(listener.Addr().String(), Routes{Health: okHandler()}, discard())
	ctx, cancel := context.WithCancel(t.Context())
	done := make(chan error, 1)
	go func() { done <- server.Serve(ctx, listener) }()

	request, err := http.NewRequestWithContext(t.Context(), http.MethodGet, "http://"+listener.Addr().String()+"/healthz", nil)
	if err != nil {
		t.Fatal(err)
	}
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	_ = response.Body.Close()
	cancel()

	select {
	case err := <-done:
		if err != nil || response.StatusCode != http.StatusOK {
			t.Errorf("Serve = %v after answering %d", err, response.StatusCode)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("Serve did not stop")
	}
}
