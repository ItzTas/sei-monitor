package ui

import (
	"bytes"
	"embed"
	"html/template"
	"log/slog"
	"net/http"
	"time"

	"github.com/ItzTas/sei-monitor/internal/api/events"
	"github.com/ItzTas/sei-monitor/internal/api/resp"
	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
)

const (
	pageSize      = 50
	timeLayout    = "2006-01-02 15:04"
	contentPolicy = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'"
)

//go:embed templates
var templateFiles embed.FS

//go:embed static
var staticFiles embed.FS

type Deps struct {
	Store      events.Repository
	Monitor    events.Monitor
	Token      config.Secret
	FullDetail bool
	Location   *time.Location
	Now        func() time.Time
	Log        *slog.Logger
}

type server struct {
	events   *events.Service
	sessions sessions
	pages    *template.Template
	log      *slog.Logger
}

type eventsPage struct {
	Events []event.View
	Failed bool
}

type loginPage struct {
	Rejected bool
}

func Register(mux *http.ServeMux, deps Deps) {
	s := &server{
		events:   events.NewService(deps.Store, deps.Monitor, deps.FullDetail, deps.Location),
		sessions: sessions{token: deps.Token, now: deps.Now},
		pages:    parsePages(deps.Location),
		log:      deps.Log,
	}
	routes := map[string]http.Handler{
		"GET /ui":         redirectTo("ui/"),
		"GET /ui/{$}":     s.authorized(s.listEvents("index")),
		"GET /ui/events":  s.authorized(s.listEvents("events")),
		"GET /ui/login":   http.HandlerFunc(s.loginForm),
		"POST /ui/login":  http.HandlerFunc(s.login),
		"POST /ui/logout": http.HandlerFunc(s.logout),
		"GET /ui/static/": http.StripPrefix("/ui/", http.FileServerFS(staticFiles)),
	}
	protection := http.NewCrossOriginProtection()
	for pattern, handler := range routes {
		mux.Handle(pattern, protection.Handler(contentSecurity(handler)))
	}
}

func parsePages(location *time.Location) *template.Template {
	funcs := template.FuncMap{
		"local": func(t time.Time) string { return t.In(location).Format(timeLayout) },
	}
	return template.Must(template.New("pages").Funcs(funcs).ParseFS(templateFiles, "templates/*.html"))
}

func (s *server) authorized(next http.HandlerFunc) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if s.sessions.valid(r) {
			next(w, r)
			return
		}
		if r.Header.Get("HX-Request") == "true" {
			w.Header().Set("HX-Redirect", "login")
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		redirect(w, "login")
	})
}

func (s *server) listEvents(page string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		list, err := s.events.List(r.Context(), events.ListInput{PageInput: resp.PageInput{Limit: pageSize}})
		if err != nil {
			s.log.Error("the web interface could not list the events", "error", err)
			s.render(w, http.StatusInternalServerError, page, eventsPage{Failed: true})
			return
		}
		s.render(w, http.StatusOK, page, eventsPage{Events: list.Items})
	}
}

func (s *server) loginForm(w http.ResponseWriter, _ *http.Request) {
	s.render(w, http.StatusOK, "login", loginPage{})
}

func (s *server) login(w http.ResponseWriter, r *http.Request) {
	if !s.sessions.accepts(r.PostFormValue("token")) {
		s.render(w, http.StatusUnauthorized, "login", loginPage{Rejected: true})
		return
	}
	http.SetCookie(w, s.sessions.issue())
	redirect(w, "./")
}

func (s *server) logout(w http.ResponseWriter, _ *http.Request) {
	http.SetCookie(w, s.sessions.clear())
	redirect(w, "login")
}

func (s *server) render(w http.ResponseWriter, status int, page string, data any) {
	var body bytes.Buffer
	if err := s.pages.ExecuteTemplate(&body, page, data); err != nil {
		s.log.Error("the web interface could not render a page", "page", page, "error", err)
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.WriteHeader(status)
	_, _ = body.WriteTo(w)
}

func redirectTo(location string) http.HandlerFunc {
	return func(w http.ResponseWriter, _ *http.Request) { redirect(w, location) }
}

func redirect(w http.ResponseWriter, location string) {
	w.Header().Set("Location", location)
	w.WriteHeader(http.StatusSeeOther)
}

func contentSecurity(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Security-Policy", contentPolicy)
		next.ServeHTTP(w, r)
	})
}
