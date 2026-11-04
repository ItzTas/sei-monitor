package main

import (
	"maps"
	"strings"
	"testing"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
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
	if _, err := config.Load(config.Source{Lookup: func(name string) (string, bool) { value, ok := env[name]; return value, ok }}); err != nil {
		t.Errorf("the configuration refuses generated tokens: %v", err)
	}
}
