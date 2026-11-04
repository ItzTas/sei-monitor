package main

import (
	"maps"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/notify/notifytest"
	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
	"github.com/ItzTas/sei-monitor/internal/store/storetest"
)

func seiEnv(server *seitest.Server, overrides map[string]string) map[string]string {
	env := map[string]string{
		"SEI_URL":      server.URL(),
		"SEI_USERNAME": seitest.Username,
		"SEI_PASSWORD": seitest.Password,
		"SEI_ORG":      seitest.OrgAcronym,
		"SEI_TIMEZONE": seitest.TimeZone,
	}
	maps.Copy(env, overrides)
	return env
}

func TestOrgsListsEveryAgencySortedByAcronym(t *testing.T) {
	server := seitest.New(t)

	code, stdout, stderr := runCLI(t, map[string]string{"SEI_URL": server.URL()}, "orgs")

	if code != 0 {
		t.Fatalf("exit code = %d, stderr = %q", code, stderr)
	}
	lines := strings.Split(strings.TrimSpace(stdout), "\n")
	if len(lines) != len(seitest.DefaultOrgs())+1 || strings.Fields(lines[0])[0] != "ACRONYM" {
		t.Fatalf("output:\n%s", stdout)
	}
	if fields := strings.Fields(lines[1]); fields[0] != "ADEMO" || fields[1] != "34" {
		t.Errorf("first row = %q, want ADEMO 34", lines[1])
	}
}

func TestTestLoginShowsTheSession(t *testing.T) {
	server := seitest.New(t)

	code, stdout, stderr := runCLI(t, seiEnv(server, map[string]string{"SEI_UNITS": "sedemo/gab"}), "test-login")

	if code != 0 {
		t.Fatalf("exit code = %d, stderr = %q", code, stderr)
	}
	want := []string{
		"API:       " + server.APIURL(),
		"Agency:    SEDEMO (id 63)",
		"Login:     ok, as fulano.tal (Fulano de Tal)",
		"Versions:  SEI 5.0.5, mod-wssei 3.0.4",
		"  SEDEMO/NUPROC (current)",
		"  SEDEMO/GAB\n",
		"Watched:   SEDEMO/GAB",
	}
	for _, line := range want {
		if !strings.Contains(stdout, line) {
			t.Errorf("output does not contain %q:\n%s", line, stdout)
		}
	}
	if strings.Contains(stdout+stderr, seitest.Password) {
		t.Error("the output shows the password")
	}
}

func TestTestLoginFailures(t *testing.T) {
	tests := []struct {
		name string
		env  map[string]string
		want string
	}{
		{"wrong password", map[string]string{"SEI_PASSWORD": "wrong"}, "error: sei: /autenticar: Usuário ou Senha Inválida."},
		{"unit the user does not have", map[string]string{"SEI_UNITS": "SEDEMO/XYZ"}, `error: SEI_UNITS: unit not found among the user's units: "SEDEMO/XYZ" (yours are SEDEMO/NUPROC, SEDEMO/GAB)`},
		{"agency missing from the login page", map[string]string{"SEI_ORG": "NOPE"}, `agency not found on the SEI login page: "NOPE"`},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server := seitest.New(t)

			code, _, stderr := runCLI(t, seiEnv(server, tt.env), "test-login")

			if code != 1 || !strings.Contains(stderr, tt.want) {
				t.Fatalf("exit code = %d, stderr = %q; want 1 and %q", code, stderr, tt.want)
			}
		})
	}
}

func TestCommandsListEveryConfigurationProblem(t *testing.T) {
	code, _, stderr := runCLI(t, map[string]string{"POLL_INTERVAL": "1m"}, "test-login")

	if code != 1 {
		t.Fatalf("exit code = %d, stderr = %q", code, stderr)
	}
	for _, problem := range []string{"invalid configuration:", "SEI_URL is required", "SEI_USERNAME is required", "SEI_PASSWORD is required", "SEI_ORG is required"} {
		if !strings.Contains(stderr, problem) {
			t.Errorf("stderr does not contain %q:\n%s", problem, stderr)
		}
	}
}

func TestGenerateTokenPrintsATokenTheConfigurationAccepts(t *testing.T) {
	first, firstOut, _ := runCLI(t, nil, "generate-token")
	second, secondOut, _ := runCLI(t, nil, "generate-token")

	feed, api := strings.TrimSpace(firstOut), strings.TrimSpace(secondOut)
	if first != 0 || second != 0 || len(feed) != 43 || feed == api {
		t.Fatalf("tokens = %q (%d) and %q (%d)", feed, first, api, second)
	}
	env := map[string]string{"FEED_TOKEN": feed, "API_TOKEN": api, "SEI_URL": "https://sei.example.org", "SEI_USERNAME": "u", "SEI_PASSWORD": "p", "SEI_ORG": "1"}
	if _, err := config.Load(envSource(env)); err != nil {
		t.Errorf("the configuration refuses generated tokens: %v", err)
	}
}

func TestSendTestReportsEveryChannel(t *testing.T) {
	smtp := notifytest.NewSMTPServer(t, notifytest.SMTPPlain)
	titles := make(chan string, 1)
	ntfy := httptest.NewServer(http.HandlerFunc(func(_ http.ResponseWriter, r *http.Request) {
		titles <- r.Header.Get("Title")
	}))
	t.Cleanup(ntfy.Close)
	webhook := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	t.Cleanup(webhook.Close)
	env := map[string]string{
		"SMTP_HOST": smtp.Host, "SMTP_PORT": strconv.Itoa(smtp.Port), "SMTP_TLS": "none",
		"SMTP_FROM": "sei-monitor@example.org", "SMTP_TO": "fulano@example.org",
		"NTFY_URL":    ntfy.URL + "/sei-topic",
		"WEBHOOK_URL": webhook.URL + "/sei", "WEBHOOK_SECRET": "0123456789abcdef",
	}

	code, stdout, stderr := runCLI(t, env, "send-test")

	want := "email: ok\nntfy: ok\nwebhook: failed: the server answered 500\n"
	if code != 1 || stdout != want || !strings.Contains(stderr, "1 of 3 channels failed") {
		t.Errorf("exit code %d\nstdout:\n%s\nstderr:\n%s", code, stdout, stderr)
	}
	if mails := smtp.Mails(); len(mails) != 1 || mails[0].To[0] != "fulano@example.org" {
		t.Errorf("mails = %+v", mails)
	}
	if title := <-titles; title != "sei-monitor: test message" {
		t.Errorf("ntfy title = %q", title)
	}
}

func TestSendTestNeedsAChannel(t *testing.T) {
	code, stdout, stderr := runCLI(t, nil, "send-test")

	if code != 1 || stdout != "" || !strings.Contains(stderr, "no channel is configured") {
		t.Errorf("exit code %d, stdout %q, stderr %q", code, stdout, stderr)
	}
}

func TestSendTestReportsAnUnpairedWhatsApp(t *testing.T) {
	env := map[string]string{"DATABASE_URL": storetest.SQLiteURL(t), "WHATSAPP_RECIPIENTS": "5595991234567"}

	code, stdout, stderr := runCLI(t, env, "send-test")

	want := "whatsapp: failed: could not connect to WhatsApp: not paired with WhatsApp; run sei-monitor whatsapp-pair\n"
	if code != 1 || stdout != want || !strings.Contains(stderr, "1 of 1 channels failed") {
		t.Errorf("exit code %d\nstdout:\n%s\nstderr:\n%s", code, stdout, stderr)
	}
}

func TestWhatsAppPairRefusesABadPhoneNumber(t *testing.T) {
	env := map[string]string{"DATABASE_URL": storetest.SQLiteURL(t)}
	tests := map[string][]string{
		"without the area code": {"whatsapp-pair", "991234567"},
		"with a leading zero":   {"whatsapp-pair", "095991234567"},
		"two numbers":           {"whatsapp-pair", "5595991234567", "5595991234568"},
	}
	for name, args := range tests {
		code, stdout, stderr := runCLI(t, env, args...)
		if code != 2 || stdout != "" || stderr == "" {
			t.Errorf("%s: exit code %d, stdout %q, stderr %q", name, code, stdout, stderr)
		}
	}
}
