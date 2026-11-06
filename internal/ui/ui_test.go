package ui_test

import (
	"bytes"
	"context"
	"errors"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/httpserver"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/store"
	"github.com/ItzTas/sei-monitor/internal/ui"
)

const (
	apiToken      = "api-token-0123456789abcdefghijklmnopqrstuv"
	sessionCookie = "__Secure-sei-monitor-session"
)

var (
	t0            = time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)
	signedSession = regexp.MustCompile(`^[0-9]+\.[A-Za-z0-9_-]{43}$`)
)

type fakeRepository struct {
	events []event.Event
	err    error
}

func (r *fakeRepository) ListEvents(context.Context, store.EventFilter) ([]event.Event, error) {
	return r.events, r.err
}

type fakeMonitor struct {
	state monitor.State
}

func (m *fakeMonitor) State() monitor.State {
	return m.state
}

type fixture struct {
	repo    *fakeRepository
	monitor *fakeMonitor
	now     time.Time
	logs    *bytes.Buffer
	handler http.Handler
}

func newFixture(t *testing.T) *fixture {
	t.Helper()
	f := &fixture{
		repo: &fakeRepository{events: []event.Event{
			{ID: 2, Kind: event.ProcessLeft, ProcedureID: 9003, OccurredAt: t0, DetectedAt: t0, Payload: event.Payload{ProcessNumber: "00000.000003/2026-00", Unit: "SEDEMO/<b>NUPROC</b>"}},
			{ID: 1, Kind: event.DocumentAdded, ProcedureID: 9001, OccurredAt: t0.Add(-2 * time.Hour), DetectedAt: t0, Payload: event.Payload{ProcessNumber: "00000.000001/2026-00", Unit: "SEDEMO/GAB"}},
		}},
		monitor: &fakeMonitor{state: monitor.State{AccountID: 1}},
		now:     t0,
		logs:    &bytes.Buffer{},
	}
	logger := slog.New(slog.NewTextHandler(f.logs, &slog.HandlerOptions{Level: slog.LevelDebug}))
	deps := ui.Deps{
		Store:    f.repo,
		Monitor:  f.monitor,
		Token:    config.NewSecret(apiToken),
		Location: time.FixedZone("UTC-3", -3*60*60),
		Now:      func() time.Time { return f.now },
		Log:      logger,
	}
	ok := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusOK) })
	f.handler = httpserver.Handler(httpserver.Routes{Health: ok, UI: func(mux *http.ServeMux) { ui.Register(mux, deps) }}, logger)
	return f
}

func (f *fixture) do(t *testing.T, request *http.Request) *httptest.ResponseRecorder {
	t.Helper()
	recorder := httptest.NewRecorder()
	f.handler.ServeHTTP(recorder, request)
	return recorder
}

func (f *fixture) get(t *testing.T, target string, session *http.Cookie) *httptest.ResponseRecorder {
	t.Helper()
	request := httptest.NewRequestWithContext(t.Context(), http.MethodGet, target, nil)
	if session != nil {
		request.AddCookie(session)
	}
	return f.do(t, request)
}

func (f *fixture) post(t *testing.T, target, form string, session *http.Cookie) *httptest.ResponseRecorder {
	t.Helper()
	request := httptest.NewRequestWithContext(t.Context(), http.MethodPost, target, strings.NewReader(form))
	request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	if session != nil {
		request.AddCookie(session)
	}
	return f.do(t, request)
}

func (f *fixture) signIn(t *testing.T) *http.Cookie {
	t.Helper()
	recorder := f.post(t, "/ui/login", "token="+apiToken, nil)
	if recorder.Code != http.StatusSeeOther {
		t.Fatalf("sign in: status %d, want %d", recorder.Code, http.StatusSeeOther)
	}
	return setCookie(t, recorder)
}

func setCookie(t *testing.T, recorder *httptest.ResponseRecorder) *http.Cookie {
	t.Helper()
	header := recorder.Header().Get("Set-Cookie")
	if header == "" {
		t.Fatal("the response sets no cookie")
	}
	cookie, err := http.ParseSetCookie(header)
	if err != nil {
		t.Fatal(err)
	}
	return cookie
}

func TestSignInWithTheAPIToken(t *testing.T) {
	f := newFixture(t)
	recorder := f.post(t, "/ui/login", "token="+apiToken, nil)

	if recorder.Code != http.StatusSeeOther || recorder.Header().Get("Location") != "./" {
		t.Fatalf("status %d, Location %q; want 303 to ./", recorder.Code, recorder.Header().Get("Location"))
	}
	cookie := setCookie(t, recorder)
	if cookie.Name != sessionCookie || !cookie.HttpOnly || !cookie.Secure || cookie.SameSite != http.SameSiteLaxMode {
		t.Errorf("cookie %+v, want %s with HttpOnly, Secure and SameSite=Lax", cookie, sessionCookie)
	}
	if cookie.Path != "" {
		t.Errorf("cookie path %q, want none so it follows the prefix of the reverse proxy", cookie.Path)
	}
	if cookie.MaxAge != int((7 * 24 * time.Hour).Seconds()) {
		t.Errorf("cookie max age %d, want 7 days", cookie.MaxAge)
	}
	if !signedSession.MatchString(cookie.Value) || !strings.HasPrefix(cookie.Value, strconv.FormatInt(t0.Add(7*24*time.Hour).Unix(), 10)+".") {
		t.Errorf("cookie value %q, want the expiry in Unix seconds, a dot and an HMAC-SHA256 signature", cookie.Value)
	}
}

func TestSignInRejectsAnotherToken(t *testing.T) {
	for name, form := range map[string]string{
		"wrong token": "token=" + strings.ToUpper(apiToken),
		"empty token": "token=",
		"no token":    "",
	} {
		t.Run(name, func(t *testing.T) {
			f := newFixture(t)
			recorder := f.post(t, "/ui/login", form, nil)

			if recorder.Code != http.StatusUnauthorized {
				t.Errorf("status %d, want %d", recorder.Code, http.StatusUnauthorized)
			}
			if recorder.Header().Get("Set-Cookie") != "" {
				t.Errorf("Set-Cookie %q, want none", recorder.Header().Get("Set-Cookie"))
			}
			if !strings.Contains(recorder.Body.String(), "That isn't the API_TOKEN from the configuration.") {
				t.Errorf("body %q, want the rejection message", recorder.Body.String())
			}
		})
	}
}

func TestSignInRejectsACrossSiteForm(t *testing.T) {
	f := newFixture(t)
	request := httptest.NewRequestWithContext(t.Context(), http.MethodPost, "/ui/login", strings.NewReader("token="+apiToken))
	request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	request.Header.Set("Sec-Fetch-Site", "cross-site")
	recorder := f.do(t, request)

	if recorder.Code != http.StatusForbidden || recorder.Header().Get("Set-Cookie") != "" {
		t.Errorf("status %d, Set-Cookie %q; want 403 without a cookie", recorder.Code, recorder.Header().Get("Set-Cookie"))
	}
}

func TestPagesNeedAValidSession(t *testing.T) {
	sessions := map[string]func(f *fixture, valid *http.Cookie) *http.Cookie{
		"no cookie": func(*fixture, *http.Cookie) *http.Cookie { return nil },
		"expired": func(f *fixture, valid *http.Cookie) *http.Cookie {
			f.now = f.now.Add(7 * 24 * time.Hour)
			return valid
		},
		"tampered expiry": func(_ *fixture, valid *http.Cookie) *http.Cookie {
			_, signature, _ := strings.Cut(valid.Value, ".")
			return &http.Cookie{Name: sessionCookie, Value: "99999999999." + signature}
		},
		"no signature": func(*fixture, *http.Cookie) *http.Cookie {
			return &http.Cookie{Name: sessionCookie, Value: "99999999999"}
		},
	}
	for name, session := range sessions {
		t.Run(name, func(t *testing.T) {
			f := newFixture(t)
			cookie := session(f, f.signIn(t))

			page := f.get(t, "/ui/", cookie)
			if page.Code != http.StatusSeeOther || page.Header().Get("Location") != "login" {
				t.Errorf("page: status %d, Location %q; want 303 to login", page.Code, page.Header().Get("Location"))
			}

			request := httptest.NewRequestWithContext(t.Context(), http.MethodGet, "/ui/events", nil)
			request.Header.Set("HX-Request", "true")
			if cookie != nil {
				request.AddCookie(cookie)
			}
			fragment := f.do(t, request)
			if fragment.Code != http.StatusUnauthorized || fragment.Header().Get("HX-Redirect") != "login" {
				t.Errorf("fragment: status %d, HX-Redirect %q; want 401 sending htmx to login", fragment.Code, fragment.Header().Get("HX-Redirect"))
			}
		})
	}
}

func TestEventsPageListsTheLatestEvents(t *testing.T) {
	f := newFixture(t)
	recorder := f.get(t, "/ui/", f.signIn(t))

	if recorder.Code != http.StatusOK {
		t.Fatalf("status %d, want %d", recorder.Code, http.StatusOK)
	}
	if got := recorder.Header().Get("Content-Security-Policy"); !strings.Contains(got, "script-src 'self'") {
		t.Errorf("Content-Security-Policy %q, want scripts limited to the same origin", got)
	}
	body := recorder.Body.String()
	for _, want := range []string{
		`<script src="static/htmx.min.js" defer></script>`,
		`hx-get="events"`,
		`<time datetime="2026-11-04T18:00:00Z">2026-11-04 15:00</time>`,
		"Process 00000.000003/2026-00 left SEDEMO/&lt;b&gt;NUPROC&lt;/b&gt;",
		"New document on process 00000.000001/2026-00",
	} {
		if !strings.Contains(body, want) {
			t.Errorf("body lacks %q:\n%s", want, body)
		}
	}
}

func TestEventsFragmentHoldsOnlyTheList(t *testing.T) {
	f := newFixture(t)
	recorder := f.get(t, "/ui/events", f.signIn(t))
	body := recorder.Body.String()

	if recorder.Code != http.StatusOK || !strings.HasPrefix(body, "<table>") {
		t.Errorf("status %d, body %q; want 200 starting with the table", recorder.Code, body)
	}
}

func TestEventsPageExplainsAnEmptyList(t *testing.T) {
	f := newFixture(t)
	f.repo.events = nil
	recorder := f.get(t, "/ui/events", f.signIn(t))

	if recorder.Code != http.StatusOK || !strings.Contains(recorder.Body.String(), "No events yet.") {
		t.Errorf("status %d, body %q; want 200 with the empty-list notice", recorder.Code, recorder.Body.String())
	}
}

func TestEventsPageReportsAFailedRead(t *testing.T) {
	f := newFixture(t)
	f.repo.err = errors.New("database is locked")
	recorder := f.get(t, "/ui/events", f.signIn(t))

	want := `<p class="notice">Couldn't load the events. The page tries again in a minute.</p>`
	if recorder.Code != http.StatusInternalServerError || recorder.Body.String() != want {
		t.Errorf("status %d, body %q; want 500 with only the failure notice", recorder.Code, recorder.Body.String())
	}
	if !strings.Contains(f.logs.String(), "database is locked") {
		t.Errorf("logs %q, want the cause", f.logs.String())
	}
}

func TestSignOutClearsTheSession(t *testing.T) {
	f := newFixture(t)
	recorder := f.post(t, "/ui/logout", "", f.signIn(t))

	if recorder.Code != http.StatusSeeOther || recorder.Header().Get("Location") != "login" {
		t.Errorf("status %d, Location %q; want 303 to login", recorder.Code, recorder.Header().Get("Location"))
	}
	cookie := setCookie(t, recorder)
	if cookie.Name != sessionCookie || cookie.MaxAge >= 0 || cookie.Value != "" {
		t.Errorf("cookie %+v, want %s deleted", cookie, sessionCookie)
	}
}

func TestPublicRoutes(t *testing.T) {
	tests := []struct {
		target   string
		status   int
		location string
		contains string
	}{
		{target: "/ui", status: http.StatusSeeOther, location: "ui/"},
		{target: "/ui/login", status: http.StatusOK, contains: `<form method="post" action="login">`},
		{target: "/ui/static/htmx.min.js", status: http.StatusOK, contains: "var htmx="},
		{target: "/ui/static/style.css", status: http.StatusOK, contains: "prefers-color-scheme"},
	}
	for _, test := range tests {
		t.Run(test.target, func(t *testing.T) {
			recorder := newFixture(t).get(t, test.target, nil)

			if recorder.Code != test.status || recorder.Header().Get("Location") != test.location {
				t.Errorf("status %d, Location %q; want %d, %q", recorder.Code, recorder.Header().Get("Location"), test.status, test.location)
			}
			if !strings.Contains(recorder.Body.String(), test.contains) {
				t.Errorf("body lacks %q", test.contains)
			}
		})
	}
}
