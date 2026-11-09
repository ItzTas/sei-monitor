package server

import (
	"crypto/subtle"
	"encoding/json"
	"log/slog"
	"net/http"
	"reflect"
	"strconv"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/danielgtaylor/huma/v2"
	"github.com/danielgtaylor/huma/v2/adapters/humago"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/modules/cycle"
	"github.com/ItzTas/sei-monitor/internal/modules/deliveries"
	"github.com/ItzTas/sei-monitor/internal/modules/events"
	"github.com/ItzTas/sei-monitor/internal/modules/processes"
	"github.com/ItzTas/sei-monitor/internal/modules/status"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/resp"
)

const APIVersion = "1.0.0"

type Monitor interface {
	State() monitor.State
	TriggerCycle() error
}

type APIDeps struct {
	Pool       *pgxpool.Pool
	Monitor    Monitor
	Token      core.Secret
	FullDetail bool
	Location   *time.Location
	Log        *slog.Logger
}

func RegisterAPI(mux *http.ServeMux, deps APIDeps) huma.API {
	api := humago.New(mux, humaConfig(deps.Log))
	api.UseMiddleware(bearerAuth(api, deps.Token))
	status.Configure(api, deps.Pool, deps.Monitor, deps.FullDetail)
	processes.Configure(api, deps.Pool, deps.Monitor, deps.FullDetail, deps.Location)
	events.Configure(api, deps.Pool, deps.Monitor, deps.FullDetail, deps.Location)
	deliveries.Configure(api, deps.Pool, deps.Monitor)
	cycle.Configure(api, deps.Monitor)
	mux.HandleFunc("GET /openapi.json", func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(api.OpenAPI())
	})
	return api
}

func Spec() ([]byte, error) {
	api := RegisterAPI(http.NewServeMux(), APIDeps{Log: slog.New(slog.DiscardHandler)})
	return json.MarshalIndent(api.OpenAPI(), "", "  ")
}

func humaConfig(log *slog.Logger) huma.Config {
	cfg := huma.DefaultConfig("sei-monitor API", APIVersion)
	cfg.OpenAPIPath, cfg.DocsPath, cfg.SchemasPath = "", "", ""
	cfg.CreateHooks = nil
	cfg.Components.Schemas = huma.NewMapRegistry("#/components/schemas/", schemaName)
	cfg.Info.Description = "What sei-monitor recorded about your SEI processes. Every route except /openapi.json needs the header Authorization: Bearer <API_TOKEN>."
	cfg.Components.SecuritySchemes = map[string]*huma.SecurityScheme{
		resp.SecurityScheme: {Type: "http", Scheme: "bearer", Description: "The API_TOKEN of the sei-monitor configuration."},
	}
	cfg.Transformers = append(cfg.Transformers, serverErrors(log))
	return cfg
}

func schemaName(t reflect.Type, hint string) string {
	renamed := map[string]string{
		"View":       "Event",
		"ViewDetail": "EventDetail",
		"ListView":   "ListEvent",
		"Detail":     "ProcessDetail",
		"Scheduled":  "CycleScheduled",
	}
	name := huma.DefaultSchemaNamer(t, hint)
	if alias, ok := renamed[name]; ok {
		return alias
	}
	return name
}

func bearerAuth(api huma.API, token core.Secret) func(huma.Context, func(huma.Context)) {
	expected := []byte("Bearer " + token.Reveal())
	return func(ctx huma.Context, next func(huma.Context)) {
		if len(ctx.Operation().Security) == 0 {
			next(ctx)
			return
		}
		if token.IsZero() || subtle.ConstantTimeCompare([]byte(ctx.Header("Authorization")), expected) != 1 {
			ctx.SetHeader("WWW-Authenticate", "Bearer")
			_ = huma.WriteErr(api, ctx, http.StatusUnauthorized, "missing or invalid API token")
			return
		}
		next(ctx)
	}
}

func serverErrors(log *slog.Logger) huma.Transformer {
	return func(ctx huma.Context, statusCode string, v any) (any, error) {
		code, err := strconv.Atoi(statusCode)
		if err != nil || code < http.StatusInternalServerError {
			return v, nil
		}
		model, ok := v.(*huma.ErrorModel)
		if !ok {
			return v, nil
		}
		attrs := []any{"status", code, "method", ctx.Method(), "route", ctx.Operation().Path}
		for _, cause := range model.Errors {
			attrs = append(attrs, "cause", cause.Message)
		}
		log.Error("API request failed", attrs...)
		clean := *model
		clean.Errors = nil
		return &clean, nil
	}
}
