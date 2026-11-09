package server

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"strings"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
)

const (
	readHeaderTimeout = 5 * time.Second
	readTimeout       = 10 * time.Second
	writeTimeout      = 30 * time.Second
	idleTimeout       = 60 * time.Second
	shutdownTimeout   = 5 * time.Second
	maxHeaderBytes    = 16 << 10
	MaxBodyBytes      = 1 << 10
)

type Routes struct {
	Health http.Handler
	Feed   http.Handler
	API    func(mux *http.ServeMux)
	UI     func(mux *http.ServeMux)
}

type Server struct {
	server *http.Server
	log    *slog.Logger
}

func New(address string, routes Routes, logger *slog.Logger) *Server {
	return &Server{
		server: &http.Server{
			Addr:              address,
			Handler:           Handler(routes, logger),
			ReadHeaderTimeout: readHeaderTimeout,
			ReadTimeout:       readTimeout,
			WriteTimeout:      writeTimeout,
			IdleTimeout:       idleTimeout,
			MaxHeaderBytes:    maxHeaderBytes,
			ErrorLog:          slog.NewLogLogger(logger.Handler(), slog.LevelWarn),
		},
		log: logger,
	}
}

func Handler(routes Routes, logger *slog.Logger) http.Handler {
	mux := http.NewServeMux()
	mux.Handle("GET /healthz", routes.Health)
	if routes.Feed != nil {
		mux.Handle("GET /feed/{file}", routes.Feed)
	}
	if routes.API != nil {
		routes.API(mux)
	}
	if routes.UI != nil {
		routes.UI(mux)
	}
	return accessLog(logger, secureHeaders(limitBody(mux)))
}

func (s *Server) Handler() http.Handler {
	return s.server.Handler
}

func (s *Server) Run(ctx context.Context) error {
	var config net.ListenConfig
	listener, err := config.Listen(ctx, "tcp", s.server.Addr)
	if err != nil {
		return fmt.Errorf("server: listen on %s: %w", s.server.Addr, err)
	}
	return s.Serve(ctx, listener)
}

func (s *Server) Serve(ctx context.Context, listener net.Listener) error {
	s.log.Info("HTTP server listening", "address", listener.Addr().String())
	errs := make(chan error, 1)
	go func() { errs <- s.server.Serve(listener) }()
	select {
	case err := <-errs:
		return err
	case <-ctx.Done():
	}
	shutdownCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), shutdownTimeout)
	defer cancel()
	if err := s.server.Shutdown(shutdownCtx); err != nil {
		return fmt.Errorf("server: shut down: %w", err)
	}
	if err := <-errs; !errors.Is(err, http.ErrServerClosed) {
		return err
	}
	return nil
}

type LastRun func(ctx context.Context) (core.CycleRun, bool, error)

func Health(lastRun LastRun, interval time.Duration, startedAt time.Time, now func() time.Time, logger *slog.Logger) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		run, found, err := lastRun(r.Context())
		if err != nil {
			logger.Error("the health check could not read the last cycle", "error", err)
		}
		ok := err == nil && healthy(run, found, now(), startedAt, interval)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(map[bool]int{true: http.StatusOK, false: http.StatusServiceUnavailable}[ok])
		_ = json.NewEncoder(w).Encode(map[string]bool{"ok": ok})
	})
}

func healthy(run core.CycleRun, found bool, now, startedAt time.Time, interval time.Duration) bool {
	finished := found && !run.FinishedAt.IsZero()
	if (!finished || run.FinishedAt.Before(startedAt)) && now.Sub(startedAt) < interval {
		return true
	}
	return finished && run.Error == "" && now.Sub(run.FinishedAt) < 3*interval
}

func secureHeaders(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		w.Header().Set("X-Content-Type-Options", "nosniff")
		next.ServeHTTP(w, r)
	})
}

func limitBody(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		r.Body = http.MaxBytesReader(w, r.Body, MaxBodyBytes)
		next.ServeHTTP(w, r)
	})
}

type statusRecorder struct {
	http.ResponseWriter
	status int
	wrote  bool
}

func (r *statusRecorder) WriteHeader(code int) {
	if !r.wrote {
		r.status, r.wrote = code, true
	}
	r.ResponseWriter.WriteHeader(code)
}

func (r *statusRecorder) Write(body []byte) (int, error) {
	r.wrote = true
	return r.ResponseWriter.Write(body)
}

func (r *statusRecorder) Unwrap() http.ResponseWriter {
	return r.ResponseWriter
}

func accessLog(logger *slog.Logger, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		started := time.Now()
		recorder := &statusRecorder{ResponseWriter: w, status: http.StatusOK}
		next.ServeHTTP(recorder, r)
		level := slog.LevelInfo
		if r.URL.Path == "/healthz" {
			level = slog.LevelDebug
		}
		logger.LogAttrs(r.Context(), level, "HTTP request",
			slog.String("method", r.Method),
			slog.String("path", maskPath(r.URL.Path)),
			slog.Int("status", recorder.status),
			slog.Duration("duration", time.Since(started).Round(time.Millisecond)),
		)
	})
}

func maskPath(path string) string {
	if before, _, found := strings.Cut(path, "/feed/"); found {
		return before + "/feed/[redacted]"
	}
	return path
}
