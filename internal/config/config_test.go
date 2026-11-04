package config

import (
	"errors"
	"io/fs"
	"log/slog"
	"maps"
	"os"
	"slices"
	"strings"
	"testing"
	"time"
)

func source(env map[string]string, files map[string]string) Source {
	return Source{
		Lookup: func(name string) (string, bool) {
			value, ok := env[name]
			return value, ok
		},
		ReadFile: func(path string) ([]byte, error) {
			content, ok := files[path]
			if !ok {
				return nil, fs.ErrNotExist
			}
			return []byte(content), nil
		},
	}
}

func baseEnv(overrides map[string]string) map[string]string {
	env := map[string]string{
		"SEI_URL":      "https://sei.example.org",
		"SEI_USERNAME": "fulano.tal",
		"SEI_PASSWORD": "p4ss",
		"SEI_ORG":      "SEDEMO",
		"FEED_TOKEN":   strings.Repeat("a", 32),
	}
	maps.Copy(env, overrides)
	for name, value := range overrides {
		if value == "<unset>" {
			delete(env, name)
		}
	}
	return env
}

func load(t *testing.T, env map[string]string, files map[string]string) (Config, []string) {
	t.Helper()
	cfg, err := Load(source(env, files))
	if err == nil {
		return cfg, nil
	}
	configErr, ok := errors.AsType[*Error](err)
	if !ok {
		t.Fatalf("error = %v, want *config.Error", err)
	}
	return cfg, configErr.Problems
}

func password(t *testing.T, sei SEI) string {
	t.Helper()
	secret, err := sei.Password.Password(t.Context())
	if err != nil {
		t.Fatal(err)
	}
	return secret.Reveal()
}

func TestLoadAppliesTheDefaults(t *testing.T) {
	cfg, problems := load(t, baseEnv(nil), nil)
	if problems != nil {
		t.Fatal(problems)
	}

	if cfg.SEI.URL != "https://sei.example.org" || cfg.SEI.APIURL != "" || cfg.SEI.Username != "fulano.tal" || cfg.SEI.Org != "SEDEMO" {
		t.Errorf("SEI = %+v", cfg.SEI)
	}
	if password(t, cfg.SEI) != "p4ss" {
		t.Error("the password does not come from SEI_PASSWORD")
	}
	if cfg.SEI.Location.String() != "America/Sao_Paulo" || cfg.SEI.Concurrency != 3 || cfg.SEI.AllUnits || cfg.SEI.Units != nil {
		t.Errorf("SEI defaults = %+v", cfg.SEI)
	}
	if cfg.DatabaseURL.Reveal() != DefaultDatabaseURL || cfg.PollInterval != 15*time.Minute || cfg.ReturnWarningDays != 2 {
		t.Errorf("defaults = database %v, interval %v, warning days %d", cfg.DatabaseURL, cfg.PollInterval, cfg.ReturnWarningDays)
	}
	if cfg.HTTP.Address != ":8080" || cfg.FullDetail || cfg.LogLevel != slog.LevelInfo {
		t.Errorf("defaults = HTTP %q, full detail %v, log level %v", cfg.HTTP.Address, cfg.FullDetail, cfg.LogLevel)
	}
	if cfg.Channels != (Channels{}) {
		t.Errorf("channels = %+v, want all off", cfg.Channels)
	}
}

func TestSEIURL(t *testing.T) {
	tests := []struct {
		value   string
		want    string
		problem string
	}{
		{"https://sei.example.org/", "https://sei.example.org", ""},
		{"https://sei.example.org/sei/", "https://sei.example.org", ""},
		{"https://portal.example.org/systems/sei", "https://portal.example.org/systems", ""},
		{"http://localhost:8080", "http://localhost:8080", ""},
		{"http://127.0.0.1:9000/sei", "http://127.0.0.1:9000", ""},
		{"http://sei.example.org", "", "SEI_URL: use https"},
		{"sei.example.org", "", "SEI_URL: use a full address"},
		{"https://sei.example.org/?a=1", "", "SEI_URL: remove everything after ? or #"},
		{"https://user:hunter2@sei.example.org", "", "SEI_URL: remove the username and password"},
		{"", "", "SEI_URL is required"},
	}
	for _, tt := range tests {
		t.Run(tt.value, func(t *testing.T) {
			cfg, problems := load(t, baseEnv(map[string]string{"SEI_URL": tt.value}), nil)
			if tt.problem == "" {
				if problems != nil || cfg.SEI.URL != tt.want {
					t.Errorf("URL = %q, problems %q; want %q", cfg.SEI.URL, problems, tt.want)
				}
				return
			}
			if !slices.ContainsFunc(problems, func(p string) bool { return strings.HasPrefix(p, tt.problem) }) {
				t.Errorf("problems = %q, want one starting with %q", problems, tt.problem)
			}
		})
	}
}

func TestPasswordSources(t *testing.T) {
	files := map[string]string{"/run/secrets/sei": "from-file\n", "/run/secrets/empty": ""}
	helper := os.Args[0]
	tests := []struct {
		name    string
		env     map[string]string
		want    string
		command bool
		problem string
	}{
		{name: "file without the final newline", env: map[string]string{"SEI_PASSWORD": "<unset>", "SEI_PASSWORD_FILE": "/run/secrets/sei"}, want: "from-file"},
		{name: "command", env: map[string]string{"SEI_PASSWORD": "<unset>", "SEI_PASSWORD_CMD": "'" + helper + "' --flag"}, command: true},
		{name: "two sources", env: map[string]string{"SEI_PASSWORD_FILE": "/run/secrets/sei"}, problem: "set only one of SEI_PASSWORD, SEI_PASSWORD_FILE and SEI_PASSWORD_CMD"},
		{name: "no source", env: map[string]string{"SEI_PASSWORD": "<unset>"}, problem: "SEI_PASSWORD is required"},
		{name: "missing file", env: map[string]string{"SEI_PASSWORD": "<unset>", "SEI_PASSWORD_FILE": "/run/secrets/missing"}, problem: "SEI_PASSWORD_FILE: /run/secrets/missing does not exist"},
		{name: "empty file", env: map[string]string{"SEI_PASSWORD": "<unset>", "SEI_PASSWORD_FILE": "/run/secrets/empty"}, problem: "SEI_PASSWORD_FILE: /run/secrets/empty is empty"},
		{name: "unknown program", env: map[string]string{"SEI_PASSWORD": "<unset>", "SEI_PASSWORD_CMD": "no-such-password-manager get"}, problem: "SEI_PASSWORD_CMD: program not found: no-such-password-manager"},
		{name: "broken quotes", env: map[string]string{"SEI_PASSWORD": "<unset>", "SEI_PASSWORD_CMD": `pass show "sei`}, problem: "SEI_PASSWORD_CMD: unterminated quote"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg, problems := load(t, baseEnv(tt.env), files)
			if tt.problem != "" {
				if !slices.ContainsFunc(problems, func(p string) bool { return strings.HasPrefix(p, tt.problem) }) {
					t.Errorf("problems = %q, want one starting with %q", problems, tt.problem)
				}
				return
			}
			if problems != nil {
				t.Fatal(problems)
			}
			if _, isCommand := cfg.SEI.Password.(*PasswordCommand); isCommand != tt.command {
				t.Fatalf("password source = %T", cfg.SEI.Password)
			}
			if !tt.command && password(t, cfg.SEI) != tt.want {
				t.Errorf("password = %q, want %q", password(t, cfg.SEI), tt.want)
			}
		})
	}
}

func TestValuesOutsideTheirLimits(t *testing.T) {
	_, problems := load(t, baseEnv(map[string]string{
		"POLL_INTERVAL":       "4m",
		"SEI_CONCURRENCY":     "50",
		"RETURN_WARNING_DAYS": "-1",
		"SEI_TIMEZONE":        "America/Atlantis",
		"NOTIFICATION_DETAIL": "everything",
		"LOG_LEVEL":           "loud",
		"HTTP_ADDRESS":        "8080",
		"DATABASE_URL":        "mysql://localhost/sei",
	}), nil)

	want := []string{
		"POLL_INTERVAL: the minimum is 5m0s",
		"SEI_CONCURRENCY: use a whole number from 1 to 10",
		"RETURN_WARNING_DAYS: use a whole number from 0 to 60",
		"SEI_TIMEZONE: unknown time zone",
		"NOTIFICATION_DETAIL: use minimal or full",
		"LOG_LEVEL: use debug, info, warn or error",
		"HTTP_ADDRESS: use host:port or :port",
		"DATABASE_URL: use sqlite:///path/to/file.db or postgres://",
	}
	for _, prefix := range want {
		if !slices.ContainsFunc(problems, func(p string) bool { return strings.HasPrefix(p, prefix) }) {
			t.Errorf("missing a problem starting with %q in %q", prefix, problems)
		}
	}
}

func TestOptionsWithinLimits(t *testing.T) {
	cfg, problems := load(t, baseEnv(map[string]string{
		"POLL_INTERVAL":       "1h",
		"SEI_UNITS":           "SEDEMO/NUPROC, sedemo/nuproc ,SEDEMO/GAB,",
		"SEI_TIMEZONE":        "America/Boa_Vista",
		"NOTIFICATION_DETAIL": "FULL",
		"LOG_LEVEL":           "debug",
		"HTTP_ADDRESS":        "",
		"FEED_TOKEN":          "<unset>",
		"DATABASE_URL":        "postgres://monitor:pw@db:5432/monitor",
	}), nil)
	if problems != nil {
		t.Fatal(problems)
	}

	if cfg.PollInterval != time.Hour || !cfg.FullDetail || cfg.LogLevel != slog.LevelDebug || cfg.SEI.Location.String() != "America/Boa_Vista" {
		t.Errorf("config = %+v", cfg)
	}
	if !slices.Equal(cfg.SEI.Units, []string{"SEDEMO/NUPROC", "SEDEMO/GAB"}) || cfg.SEI.AllUnits {
		t.Errorf("units = %q (all %v)", cfg.SEI.Units, cfg.SEI.AllUnits)
	}
	if cfg.HTTP.Enabled() {
		t.Errorf("HTTP = %+v, want it off when HTTP_ADDRESS is empty", cfg.HTTP)
	}
	if all, _ := load(t, baseEnv(map[string]string{"SEI_UNITS": "All"}), nil); !all.SEI.AllUnits {
		t.Error("SEI_UNITS=All does not select every unit")
	}
}

func TestFeedToken(t *testing.T) {
	tests := map[string]string{
		"":                       "FEED_TOKEN is required while the HTTP server is on",
		"short-token":            "FEED_TOKEN: use at least 32 characters",
		strings.Repeat("a/", 20): "FEED_TOKEN: use only letters, digits",
	}
	for token, problem := range tests {
		_, problems := load(t, baseEnv(map[string]string{"FEED_TOKEN": token}), nil)
		if !slices.ContainsFunc(problems, func(p string) bool { return strings.HasPrefix(p, problem) }) {
			t.Errorf("FEED_TOKEN %q: problems = %q, want %q", token, problems, problem)
		}
	}
}

func TestChannelsTurnOnWithTheirMainVariable(t *testing.T) {
	cfg, problems := load(t, baseEnv(map[string]string{
		"SMTP_HOST":           "smtp.example.org",
		"SMTP_USERNAME":       "monitor",
		"SMTP_PASSWORD":       "smtp-pw",
		"SMTP_FROM":           "sei-monitor <monitor@example.org>",
		"SMTP_TO":             "ana@example.org, bia@example.org",
		"SMTP_TLS":            "tls",
		"TELEGRAM_TOKEN":      "123456789:AAEabcdefghijklmnopqrstuvwx",
		"TELEGRAM_CHAT_ID":    "-1001234567890",
		"NTFY_URL":            "https://ntfy.example.org/sei-topic/",
		"WEBHOOK_URL":         "https://hooks.example.org/in?key=abc",
		"WEBHOOK_SECRET":      "0123456789abcdef",
		"WHATSAPP_RECIPIENTS": "+55 (95) 99123-4567, 120363012345678901@g.us",
	}), nil)
	if problems != nil {
		t.Fatal(problems)
	}

	email := cfg.Channels.Email
	if email == nil || email.Port != 465 || email.TLS != TLSImplicit || email.Password.Reveal() != "smtp-pw" || !slices.Equal(email.To, []string{"ana@example.org", "bia@example.org"}) {
		t.Errorf("email = %+v", email)
	}
	if cfg.Channels.Telegram == nil || cfg.Channels.Telegram.ChatID != "-1001234567890" {
		t.Errorf("telegram = %+v", cfg.Channels.Telegram)
	}
	if cfg.Channels.Ntfy == nil || cfg.Channels.Ntfy.URL != "https://ntfy.example.org/sei-topic" || !cfg.Channels.Ntfy.Token.IsZero() {
		t.Errorf("ntfy = %+v", cfg.Channels.Ntfy)
	}
	if cfg.Channels.Webhook == nil || cfg.Channels.Webhook.URL != "https://hooks.example.org/in?key=abc" {
		t.Errorf("webhook = %+v", cfg.Channels.Webhook)
	}
	if cfg.Channels.WhatsApp == nil || !slices.Equal(cfg.Channels.WhatsApp.Recipients, []string{"5595991234567", "120363012345678901@g.us"}) {
		t.Errorf("whatsapp = %+v", cfg.Channels.WhatsApp)
	}
}

func TestIncompleteChannelsStopTheStart(t *testing.T) {
	_, problems := load(t, baseEnv(map[string]string{
		"SMTP_HOST":           "smtp.example.org",
		"SMTP_USERNAME":       "monitor",
		"SMTP_TLS":            "none",
		"SMTP_FROM":           "not an address",
		"TELEGRAM_TOKEN":      "abc",
		"NTFY_URL":            "https://ntfy.example.org",
		"WEBHOOK_URL":         "https://hooks.example.org/in",
		"WEBHOOK_SECRET":      "short",
		"WHATSAPP_RECIPIENTS": "123, grupo@g.us",
	}), nil)

	want := []string{
		"SMTP_TO is required",
		"SMTP_FROM: invalid email address",
		"SMTP_USERNAME and SMTP_PASSWORD go together",
		"TELEGRAM_TOKEN: this does not look like a bot token",
		"TELEGRAM_CHAT_ID is required",
		"NTFY_URL: end the address with the topic",
		"WEBHOOK_SECRET: use at least 16 characters",
		`WHATSAPP_RECIPIENTS: "123" is neither`,
		`WHATSAPP_RECIPIENTS: "grupo@g.us" is neither`,
	}
	for _, prefix := range want {
		if !slices.ContainsFunc(problems, func(p string) bool { return strings.HasPrefix(p, prefix) }) {
			t.Errorf("missing a problem starting with %q in %q", prefix, problems)
		}
	}
}

func TestUnencryptedSMTPRefusesAPassword(t *testing.T) {
	_, problems := load(t, baseEnv(map[string]string{
		"SMTP_HOST": "smtp.example.org", "SMTP_USERNAME": "monitor", "SMTP_PASSWORD": "smtp-pw",
		"SMTP_FROM": "monitor@example.org", "SMTP_TO": "ana@example.org", "SMTP_TLS": "none",
	}), nil)

	if !slices.Contains(problems, "SMTP_TLS=none would send the password unencrypted; use starttls or tls") {
		t.Errorf("problems = %q", problems)
	}
}

func TestProblemsNeverShowSecretValues(t *testing.T) {
	secrets := map[string]string{
		"SEI_PASSWORD":   "sei-secret-1",
		"FEED_TOKEN":     "feed-secret-2",
		"TELEGRAM_TOKEN": "telegram-secret-3",
		"WEBHOOK_SECRET": "hook-4",
		"DATABASE_URL":   "mysql://user:database-secret-5@host/db",
		"SEI_URL":        "https://user:url-secret-6@sei.example.org",
	}
	env := baseEnv(secrets)
	env["SEI_PASSWORD_FILE"] = "/run/secrets/sei"
	env["WEBHOOK_URL"] = "https://hooks.example.org/in"

	_, err := Load(source(env, map[string]string{"/run/secrets/sei": "file-secret-7"}))
	if err == nil {
		t.Fatal("want an error")
	}

	for _, value := range []string{"sei-secret-1", "feed-secret-2", "telegram-secret-3", "hook-4", "database-secret-5", "url-secret-6", "file-secret-7"} {
		if strings.Contains(err.Error(), value) {
			t.Errorf("the error shows %q:\n%v", value, err)
		}
	}
}
