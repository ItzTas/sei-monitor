package core

import (
	"errors"
	"io/fs"
	"maps"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/goldentest"
)

func TestGoldenVectors(t *testing.T) {
	goldentest.Golden(t, "config.json", configVectors(t))
	goldentest.Golden(t, "sei_url.json", seiURLVectors(t))
	goldentest.Golden(t, "duration.json", durationVectors())
	goldentest.Golden(t, "split_command.json", splitCommandVectors())
	goldentest.Golden(t, "without_secrets.json", withoutSecretsVectors())
	goldentest.Golden(t, "go_text.json", goTextVectors())
	goldentest.Golden(t, "local_time.json", localTimeVectors(t))
}

func errorText(err error) *string {
	if err == nil {
		return nil
	}
	text := err.Error()
	return &text
}

func problemsOf(t *testing.T, err error) []string {
	t.Helper()
	if err == nil {
		return nil
	}
	configErr, ok := errors.AsType[*Error](err)
	if !ok {
		t.Fatalf("error = %v, want *core.Error", err)
	}
	return configErr.Problems
}

type configCase struct {
	Name     string            `json:"name"`
	Env      map[string]string `json:"env"`
	Files    map[string]string `json:"files"`
	Result   any               `json:"result"`
	Problems []string          `json:"problems"`
}

type configVectorSet struct {
	Plugin []configCase `json:"plugin"`
	SEI    []configCase `json:"sei"`
	SEIURL []configCase `json:"sei_url"`
}

type passwordRecord struct {
	Source string   `json:"source"`
	Value  *string  `json:"value,omitempty"`
	Args   []string `json:"args,omitempty"`
}

type seiRecord struct {
	URL         string         `json:"url"`
	APIURL      string         `json:"api_url"`
	Username    string         `json:"username"`
	Org         string         `json:"org"`
	Units       []string       `json:"units"`
	AllUnits    bool           `json:"all_units"`
	Zone        string         `json:"zone"`
	Concurrency int            `json:"concurrency"`
	Password    passwordRecord `json:"password"`
}

type pluginRecord struct {
	SEI               seiRecord `json:"sei"`
	DatabaseURL       string    `json:"database_url"`
	PollIntervalNS    int64     `json:"poll_interval_ns"`
	ReturnWarningDays int       `json:"return_warning_days"`
	RetentionDays     int       `json:"retention_days"`
	FullDetail        bool      `json:"full_detail"`
	LogLevel          int       `json:"log_level"`
}

type configInput struct {
	name  string
	env   map[string]string
	files map[string]string
}

func minimalEnv(changes map[string]string, unset ...string) map[string]string {
	env := map[string]string{
		"SEI_URL":      "https://sei.example.org",
		"SEI_USERNAME": "user.name",
		"SEI_PASSWORD": "s3cr3t-password",
		"SEI_ORG":      "SEDEMO",
		"DATABASE_URL": "postgres://monitor:s3cr3t-db@db.example.org/monitor",
	}
	maps.Copy(env, changes)
	for _, name := range unset {
		delete(env, name)
	}
	return env
}

func goldenSource(env, files map[string]string) Source {
	lookup := maps.Clone(env)
	if _, set := lookup["HTTP_ADDRESS"]; !set {
		lookup["HTTP_ADDRESS"] = ""
	}
	return Source{
		Lookup: func(name string) (string, bool) {
			value, ok := lookup[name]
			return value, ok
		},
		ReadFile: func(path string) ([]byte, error) {
			if strings.HasPrefix(path, "/denied/") {
				return nil, fs.ErrPermission
			}
			content, ok := files[path]
			if !ok {
				return nil, fs.ErrNotExist
			}
			return []byte(content), nil
		},
	}
}

func passwordOf(t *testing.T, source PasswordSource) passwordRecord {
	t.Helper()
	switch password := source.(type) {
	case staticPassword:
		value := password.secret.Reveal()
		return passwordRecord{Source: "static", Value: &value}
	case *PasswordCommand:
		return passwordRecord{Source: "command", Args: password.args}
	}
	t.Fatalf("password source %T", source)
	return passwordRecord{}
}

func seiRecordOf(t *testing.T, sei SEI) seiRecord {
	t.Helper()
	units := sei.Units
	if units == nil {
		units = []string{}
	}
	return seiRecord{
		URL:         sei.URL,
		APIURL:      sei.APIURL,
		Username:    sei.Username,
		Org:         sei.Org,
		Units:       units,
		AllUnits:    sei.AllUnits,
		Zone:        sei.Location.String(),
		Concurrency: sei.Concurrency,
		Password:    passwordOf(t, sei.Password),
	}
}

func fakeProgramPath(t *testing.T) {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "sh"), []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", dir)
}

func configVectors(t *testing.T) configVectorSet {
	fakeProgramPath(t)
	var set configVectorSet
	for _, input := range configInputs() {
		files := input.files
		if files == nil {
			files = map[string]string{}
		}
		src := goldenSource(input.env, files)
		base := configCase{Name: input.name, Env: input.env, Files: files}

		plugin := base
		cfg, err := Load(src)
		plugin.Problems = problemsOf(t, err)
		if err == nil {
			plugin.Result = pluginRecord{
				SEI:               seiRecordOf(t, cfg.SEI),
				DatabaseURL:       cfg.DatabaseURL.Reveal(),
				PollIntervalNS:    int64(cfg.PollInterval),
				ReturnWarningDays: cfg.ReturnWarningDays,
				RetentionDays:     cfg.RetentionDays,
				FullDetail:        cfg.FullDetail,
				LogLevel:          int(cfg.LogLevel),
			}
		}
		set.Plugin = append(set.Plugin, plugin)

		seiCase := base
		sei, err := LoadSEI(src)
		seiCase.Problems = problemsOf(t, err)
		if err == nil {
			seiCase.Result = seiRecordOf(t, sei)
		}
		set.SEI = append(set.SEI, seiCase)

		urlCase := base
		seiURL, err := LoadSEIURL(src)
		urlCase.Problems = problemsOf(t, err)
		if err == nil {
			urlCase.Result = seiURL
		}
		set.SEIURL = append(set.SEIURL, urlCase)
	}
	return set
}

func configInputs() []configInput {
	secretFiles := map[string]string{
		"/run/secrets/sei":           "pw\n",
		"/run/secrets/sei-crlf":      "pw\r\n",
		"/run/secrets/sei-two-lines": "pw\n\n",
		"/run/secrets/empty":         "",
		"/run/secrets/database":      "postgres://monitor:s3cr3t-file@db.example.org/monitor\n",
		"/run/secrets/bad-database":  "mysql://user:file-db-s3cr3t@db.example.org/sei",
	}
	inputs := []configInput{
		{name: "defaults with the minimum required", env: minimalEnv(nil)},
		{name: "empty environment", env: map[string]string{}},
		{name: "only POLL_INTERVAL=1m", env: map[string]string{"POLL_INTERVAL": "1m"}},
		{name: "a problem in every SEI and plugin setting", env: minimalEnv(map[string]string{
			"SEI_URL":             "ftp://sei.example.org",
			"SEI_API_URL":         "https://sei.example.org/api?x=1",
			"SEI_TIMEZONE":        "Mars/Base",
			"SEI_CONCURRENCY":     "0",
			"DATABASE_URL":        "mysql://db.example.org/sei",
			"POLL_INTERVAL":       "4m",
			"RETURN_WARNING_DAYS": "61",
			"RETENTION_DAYS":      "3651",
			"NOTIFICATION_DETAIL": "verbose",
			"LOG_LEVEL":           "loud",
		}, "SEI_USERNAME", "SEI_PASSWORD", "SEI_ORG")},
		{name: "SEI_URL with /sei/", env: minimalEnv(map[string]string{"SEI_URL": "https://sei.example.org/sei/"})},
		{name: "SEI_URL over plain http", env: minimalEnv(map[string]string{"SEI_URL": "http://sei.example.org"})},
		{name: "SEI_API_URL valid", env: minimalEnv(map[string]string{"SEI_API_URL": "https://sei.example.org/sei/modulos/wssei/controlador_ws.php/api/v2/"})},
		{name: "SEI_API_URL on localhost over http", env: minimalEnv(map[string]string{"SEI_API_URL": "http://localhost:8080/api/"})},
		{name: "SEI_API_URL with a query", env: minimalEnv(map[string]string{"SEI_API_URL": "https://sei.example.org/api/v2?x=1"})},
		{name: "SEI_API_URL over plain http", env: minimalEnv(map[string]string{"SEI_API_URL": "http://sei.example.org/api/v2"})},
		{name: "SEI_USERNAME with spaces", env: minimalEnv(map[string]string{"SEI_USERNAME": " fulano.tal "})},
		{name: "SEI_ORG numeric", env: minimalEnv(map[string]string{"SEI_ORG": "63"})},
		{name: "SEI_ORG with spaces", env: minimalEnv(map[string]string{"SEI_ORG": "  sedemo "})},
		{name: "SEI_ORG only spaces", env: minimalEnv(map[string]string{"SEI_ORG": "   "})},
	}
	for _, value := range []string{"0", "1", "10", "11", "3.0", "+3", "03", "-1", " 7 "} {
		inputs = append(inputs, configInput{name: "SEI_CONCURRENCY=" + value, env: minimalEnv(map[string]string{"SEI_CONCURRENCY": value})})
	}
	for _, value := range []string{"-1", "0", "60", "61", "x"} {
		inputs = append(inputs, configInput{name: "RETURN_WARNING_DAYS=" + value, env: minimalEnv(map[string]string{"RETURN_WARNING_DAYS": value})})
	}
	for _, value := range []string{"0", "3", "4", "3650", "3651", "x", "-1"} {
		inputs = append(inputs, configInput{name: "RETENTION_DAYS=" + value, env: minimalEnv(map[string]string{"RETENTION_DAYS": value})})
	}
	for _, pair := range [][2]string{{"31", "30"}, {"32", "30"}, {"3", "x"}, {"4", "x"}, {"x", "30"}, {"1", "0"}, {"2", "0"}, {"0", "60"}, {"61", "60"}, {"62", "60"}} {
		inputs = append(inputs, configInput{
			name: "RETENTION_DAYS=" + pair[0] + " with RETURN_WARNING_DAYS=" + pair[1],
			env:  minimalEnv(map[string]string{"RETENTION_DAYS": pair[0], "RETURN_WARNING_DAYS": pair[1]}),
		})
	}
	for _, value := range []string{"15m", "1h30m", "90s", "300s", "4m59s", "5m", "15", "-5m", ".5h", " 10m ", "4m59.999999999s"} {
		inputs = append(inputs, configInput{name: "POLL_INTERVAL=" + value, env: minimalEnv(map[string]string{"POLL_INTERVAL": value})})
	}
	for _, value := range []string{"FULL", "minimal", "verbose", " full "} {
		inputs = append(inputs, configInput{name: "NOTIFICATION_DETAIL=" + value, env: minimalEnv(map[string]string{"NOTIFICATION_DETAIL": value})})
	}
	for _, value := range []string{"debug", "INFO", "warn", "warning", "error", "debug+2", "info-4", "ERROR+10", "loud", "", "info+", "-4"} {
		inputs = append(inputs, configInput{name: "LOG_LEVEL=" + value, env: minimalEnv(map[string]string{"LOG_LEVEL": value})})
	}
	for _, value := range []string{"America/Boa_Vista", "UTC", "Brazil/East", "america/boa_vista", "Mars/Base", " America/Boa_Vista ", "", "../etc/passwd", "/etc/localtime"} {
		inputs = append(inputs, configInput{name: "SEI_TIMEZONE=" + value, env: minimalEnv(map[string]string{"SEI_TIMEZONE": value})})
	}
	for _, value := range []string{"", "all", "ALL", " all ", " a , A ,b,", ",", "SEDEMO/NUPROC", "Ó/GAB, ó/gab ,ção", "\u212a,k"} {
		inputs = append(inputs, configInput{name: "SEI_UNITS=" + value, env: minimalEnv(map[string]string{"SEI_UNITS": value})})
	}
	inputs = append(inputs, passwordInputs(secretFiles)...)
	inputs = append(inputs, databaseInputs(secretFiles)...)
	inputs = append(inputs, secretProblemInputs(secretFiles)...)
	return inputs
}

func passwordInputs(files map[string]string) []configInput {
	withoutDirect := func(changes map[string]string) map[string]string {
		return minimalEnv(changes, "SEI_PASSWORD")
	}
	return []configInput{
		{name: "no password", env: withoutDirect(nil)},
		{name: "SEI_PASSWORD only spaces", env: minimalEnv(map[string]string{"SEI_PASSWORD": "   "})},
		{name: "SEI_PASSWORD keeps its spaces", env: minimalEnv(map[string]string{"SEI_PASSWORD": " pw "})},
		{name: "SEI_PASSWORD_FILE ending in a newline", env: withoutDirect(map[string]string{"SEI_PASSWORD_FILE": "/run/secrets/sei"}), files: files},
		{name: "SEI_PASSWORD_FILE ending in CRLF", env: withoutDirect(map[string]string{"SEI_PASSWORD_FILE": "/run/secrets/sei-crlf"}), files: files},
		{name: "SEI_PASSWORD_FILE ending in two newlines", env: withoutDirect(map[string]string{"SEI_PASSWORD_FILE": "/run/secrets/sei-two-lines"}), files: files},
		{name: "SEI_PASSWORD_FILE empty", env: withoutDirect(map[string]string{"SEI_PASSWORD_FILE": "/run/secrets/empty"}), files: files},
		{name: "SEI_PASSWORD_FILE missing", env: withoutDirect(map[string]string{"SEI_PASSWORD_FILE": "/run/secrets/missing"}), files: files},
		{name: "SEI_PASSWORD_FILE denied", env: withoutDirect(map[string]string{"SEI_PASSWORD_FILE": "/denied/sei"}), files: files},
		{name: "SEI_PASSWORD_FILE path with spaces", env: withoutDirect(map[string]string{"SEI_PASSWORD_FILE": " /run/secrets/sei "}), files: files},
		{name: "SEI_PASSWORD_FILE only spaces", env: withoutDirect(map[string]string{"SEI_PASSWORD_FILE": "   "}), files: files},
		{name: "SEI_PASSWORD and SEI_PASSWORD_FILE", env: minimalEnv(map[string]string{"SEI_PASSWORD_FILE": "/run/secrets/sei"}), files: files},
		{name: "SEI_PASSWORD and SEI_PASSWORD_CMD", env: minimalEnv(map[string]string{"SEI_PASSWORD_CMD": "sh -c 'echo x'"})},
		{name: "all three password sources", env: minimalEnv(map[string]string{"SEI_PASSWORD_FILE": "/run/secrets/sei", "SEI_PASSWORD_CMD": "sh -c 'echo x'"}), files: files},
		{name: "SEI_PASSWORD_CMD runs a program on PATH", env: withoutDirect(map[string]string{"SEI_PASSWORD_CMD": "sh -c 'echo x'"})},
		{name: "SEI_PASSWORD_CMD unknown program", env: withoutDirect(map[string]string{"SEI_PASSWORD_CMD": "no-such-program-sei-monitor"})},
		{name: "SEI_PASSWORD_CMD unknown absolute program", env: withoutDirect(map[string]string{"SEI_PASSWORD_CMD": "/no/such/program-sei-monitor get"})},
		{name: "SEI_PASSWORD_CMD unterminated quote", env: withoutDirect(map[string]string{"SEI_PASSWORD_CMD": "'aberto"})},
		{name: "SEI_PASSWORD_CMD trailing backslash", env: withoutDirect(map[string]string{"SEI_PASSWORD_CMD": `sh \`})},
		{name: "SEI_PASSWORD_CMD empty program name", env: withoutDirect(map[string]string{"SEI_PASSWORD_CMD": `""`})},
		{name: "SEI_PASSWORD_CMD only spaces", env: withoutDirect(map[string]string{"SEI_PASSWORD_CMD": "   "})},
	}
}

func databaseInputs(files map[string]string) []configInput {
	return []configInput{
		{name: "DATABASE_URL missing", env: minimalEnv(nil, "DATABASE_URL")},
		{name: "DATABASE_URL only spaces", env: minimalEnv(map[string]string{"DATABASE_URL": "   "})},
		{name: "DATABASE_URL mysql", env: minimalEnv(map[string]string{"DATABASE_URL": "mysql://x"})},
		{name: "DATABASE_URL scheme only", env: minimalEnv(map[string]string{"DATABASE_URL": "postgres://"})},
		{name: "DATABASE_URL postgresql scheme", env: minimalEnv(map[string]string{"DATABASE_URL": "postgresql://u:p@h/db"})},
		{name: "DATABASE_URL upper-case scheme", env: minimalEnv(map[string]string{"DATABASE_URL": "POSTGRES://u:p@h/db"})},
		{name: "DATABASE_URL with spaces around", env: minimalEnv(map[string]string{"DATABASE_URL": " postgresql://u:p@h/db "})},
		{name: "DATABASE_URL_FILE", env: minimalEnv(map[string]string{"DATABASE_URL_FILE": "/run/secrets/database"}, "DATABASE_URL"), files: files},
		{name: "DATABASE_URL and DATABASE_URL_FILE", env: minimalEnv(map[string]string{"DATABASE_URL_FILE": "/run/secrets/database"}), files: files},
		{name: "DATABASE_URL_FILE empty", env: minimalEnv(map[string]string{"DATABASE_URL_FILE": "/run/secrets/empty"}, "DATABASE_URL"), files: files},
		{name: "DATABASE_URL_FILE missing", env: minimalEnv(map[string]string{"DATABASE_URL_FILE": "/run/secrets/missing"}, "DATABASE_URL"), files: files},
		{name: "DATABASE_URL_FILE denied", env: minimalEnv(map[string]string{"DATABASE_URL_FILE": "/denied/database"}, "DATABASE_URL"), files: files},
	}
}

func secretProblemInputs(files map[string]string) []configInput {
	return []configInput{
		{name: "secret values in every setting with a problem", env: minimalEnv(map[string]string{
			"SEI_URL":           "https://user:url-s3cr3t@sei.example.org",
			"SEI_API_URL":       "https://user:api-s3cr3t@sei.example.org/api",
			"SEI_PASSWORD":      "pw-s3cr3t",
			"SEI_PASSWORD_FILE": "/run/secrets/sei",
			"DATABASE_URL":      "mysql://user:db-s3cr3t@db.example.org/sei",
		}), files: files},
		{name: "secret values in files and commands with a problem", env: minimalEnv(map[string]string{
			"SEI_URL":           "https://sei.example.org/?token=query-s3cr3t#fragment-s3cr3t",
			"SEI_API_URL":       "https://sei.example.org/api#api-fragment-s3cr3t",
			"SEI_PASSWORD_CMD":  "pass show 'cmd-s3cr3t",
			"DATABASE_URL_FILE": "/run/secrets/bad-database",
		}, "SEI_PASSWORD", "DATABASE_URL"), files: files},
		{name: "secret values in both forms of the same setting", env: minimalEnv(map[string]string{
			"DATABASE_URL":      "postgres://user:both-db-s3cr3t@db.example.org/sei",
			"DATABASE_URL_FILE": "/run/secrets/database",
			"SEI_PASSWORD":      "both-pw-s3cr3t",
			"SEI_PASSWORD_CMD":  "sh -c 'echo cmd-s3cr3t'",
		}), files: files},
	}
}

type seiURLCase struct {
	Input      string   `json:"input"`
	Normalized *string  `json:"normalized"`
	Problems   []string `json:"problems"`
}

func seiURLVectors(t *testing.T) []seiURLCase {
	inputs := []string{
		"https://sei.rr.gov.br", "https://sei.rr.gov.br/", "https://sei.rr.gov.br/sei", "https://sei.rr.gov.br/sei/",
		"https://sei.rr.gov.br/SEI", "https://sei.rr.gov.br/sei/sei/", "https://sei.rr.gov.br/x/sei//",
		"HTTPS://SEI.RR.GOV.BR/Sei", "https://h/%73ei", "https://h/a b", "https://h/a%20b", "https://h/ção",
		"https://h/ção/sei", "https://sêi.br", "https://h:443", "https://h?", "https://h#", "https://h?x=1", "https://h#f",
		"https://u:p@h", "https://u@h", "http://localhost:8080/sei/", "http://LOCALHOST", "http://127.0.0.2", "http://[::1]:8080",
		"http://example.com", "ftp://h", "https:h", "https:///p", "https://h/%zz", "https://h{}", "  https://h/  ",
		"", "   ", "sei.rr.gov.br", "https://h/%2Fsei", "https://h/a%2fb/", "https://h/sei%2F",
	}
	cases := make([]seiURLCase, len(inputs))
	for i, input := range inputs {
		normalized, err := LoadSEIURL(goldenSource(map[string]string{"SEI_URL": input}, nil))
		cases[i] = seiURLCase{Input: input, Problems: problemsOf(t, err)}
		if err == nil {
			cases[i].Normalized = &normalized
		}
	}
	return cases
}

type durationParseCase struct {
	Input       string `json:"input"`
	Nanoseconds *int64 `json:"nanoseconds"`
}

type durationFormatCase struct {
	Nanoseconds int64  `json:"nanoseconds"`
	Text        string `json:"text"`
}

type durationVectorSet struct {
	Parse  []durationParseCase  `json:"parse"`
	Format []durationFormatCase `json:"format"`
}

func durationVectors() durationVectorSet {
	var set durationVectorSet
	for _, input := range []string{
		"0", "15m", "1h30m", "90s", "1.5h", ".5h", "5.", "300s", "1m30.5s", "1ns", "1us", "1\u00b5s", "1\u03bcs", "1ms",
		"-5m", "+5m", "5", "", "5x", "5M", "1h 30m", "9223372036854775807ns", "9223372036854775808ns", "2562047h",
		"2562048h", "0.0000000001s", "1e3s", "-0", "+0", "0s", ".", ".s", "1h1h", "1h-5m", "\u22125m", "\uff11m",
		"1.0000000009s", "-9223372036854775808ns", "-9223372036854775809ns", "4m59.999999999s", " 5m", "5m ",
	} {
		c := durationParseCase{Input: input}
		if parsed, err := time.ParseDuration(input); err == nil {
			nanoseconds := int64(parsed)
			c.Nanoseconds = &nanoseconds
		}
		set.Parse = append(set.Parse, c)
	}
	for _, duration := range []time.Duration{
		0, 1, 999, 1000, 1500000, 59 * time.Second, 5 * time.Minute, 4 * time.Minute, 90 * time.Minute,
		30 * time.Second, -90 * time.Second, time.Duration(1<<63 - 1), time.Duration(-1 << 63),
		DefaultPollInterval, 299*time.Second + 999*time.Millisecond, time.Hour, -1, 1001000,
	} {
		set.Format = append(set.Format, durationFormatCase{Nanoseconds: int64(duration), Text: duration.String()})
	}
	return set
}

type splitCommandCase struct {
	Input string   `json:"input"`
	Args  []string `json:"args"`
	Error *string  `json:"error"`
}

func splitCommandVectors() []splitCommandCase {
	inputs := []string{
		"pass show sei/monitor", "  secret-tool   lookup service sei  ", `bw get password "sei monitor"`, `cmd ''`,
		`cmd "a\"b"`, `cmd 'it''s'`, "", "   ", `cmd "open`, "cmd 'open", `cmd trailing\`,
		`a\ b`, `"a \" b"`, `'a \' b`, `""`, `''`, `a""b`, `x \`, `"open`, "a\u00a0b", "a\tb\nc\r\nd\ve\ff",
		`pass show "sei/my pass"`, "a\u0085b", "a\u3000b", "a\u2028b", "a\u200bb", "a\u1680b", "a\u202fb", "a\u205fb",
		`çãõ "ã b" 'é c' d\ é`, `"a\\b"`, `'a\\b'`, `\'`, `\"`, `\\`, `a\` + "é", `"a'b" 'a"b'`, `"" ''`, `x "" y`,
	}
	cases := make([]splitCommandCase, len(inputs))
	for i, input := range inputs {
		args, err := splitCommand(input)
		cases[i] = splitCommandCase{Input: input, Args: args, Error: errorText(err)}
	}
	return cases
}

type withoutSecretsCase struct {
	Environ []string `json:"environ"`
	Kept    []string `json:"kept"`
}

func withoutSecretsVectors() []withoutSecretsCase {
	names := []string{"SEI_PASSWORD", "FEED_TOKEN", "API_TOKEN", "SMTP_PASSWORD", "TELEGRAM_TOKEN", "NTFY_TOKEN", "WEBHOOK_SECRET", "POSTGRES_PASSWORD", "DATABASE_URL"}
	var every []string
	for _, name := range names {
		every = append(every, name+"=v", name+"_FILE=/run/secrets/x", name+"_CMD=pass show x")
	}
	every = append(every, "PATH=/usr/bin", "HOME=/home/user", "SEI_USERNAME=user.name")
	environs := [][]string{
		every,
		{},
		{"PATH=/usr/bin"},
		{"sei_password=v", "Sei_Password_File=/x", "SEI_PASSWORD_file=/x", "database_url_cmd=x"},
		{"SEI_PASSWORD_FILE_CMD=x", "DATABASE_URL_CMD_FILE=x", "SEI_PASSWORD_CMD_CMD=x", "SEI_PASSWORD_FILE_FILE=x"},
		{"AMQP_URL=amqp://user:pw@rabbit/", "SEI_URL=https://sei.example.org", "VAULT_TOKEN=x", "SEI_PASSWORDS=x", "XSEI_PASSWORD=x"},
		{"SEI_PASSWORD", "PATH", "", "=SEI_PASSWORD", "DATABASE_URL==x", "FEED_TOKEN_FILE"},
		{"\u017fei_password=v", "se\u0131_password=v", "ÇÃO=v", "çsei_password=v", "SEI_PASSWORD\u0130=v", "ção_SEI_PASSWORD=v"},
		{"PATH=/usr/bin", "PATH=/bin", "SEI_PASSWORD=a", "SEI_PASSWORD=b"},
	}
	cases := make([]withoutSecretsCase, len(environs))
	for i, environ := range environs {
		cases[i] = withoutSecretsCase{Environ: environ, Kept: withoutSecrets(environ)}
	}
	return cases
}

type quoteCase struct {
	Input  string `json:"input"`
	Output string `json:"output"`
}

type equalFoldCase struct {
	A     string `json:"a"`
	B     string `json:"b"`
	Equal bool   `json:"equal"`
}

type fieldsCase struct {
	Input  string   `json:"input"`
	Output []string `json:"output"`
}

type atoiCase struct {
	Input string `json:"input"`
	Value *int   `json:"value"`
}

type goTextVectorSet struct {
	Quote     []quoteCase     `json:"quote"`
	EqualFold []equalFoldCase `json:"equal_fold"`
	Fields    []fieldsCase    `json:"fields"`
	Lower     []quoteCase     `json:"lower"`
	Atoi      []atoiCase      `json:"atoi"`
}

func goTextVectors() goTextVectorSet {
	var set goTextVectorSet
	for _, input := range []string{
		`a"b`, `a\b`, "\t", "\n", "\x00", "\x7f", "\u00a0", "é", "\U0001f600", "\u2028", "\u200b", "",
		"\r", "\a", "\b", "\f", "\v", "\x1b", "'", "\u00ad", "\ufffd", "\ufeff", "\u0080", "\u009f", "e\u0301",
		"Mars/Base", "ção\tfim", "\U000e0001", "\U0010ffff", "\u0378", "\u3000", "ção \u00a0 x",
	} {
		set.Quote = append(set.Quote, quoteCase{Input: input, Output: strconv.Quote(input)})
	}
	for _, pair := range [][2]string{
		{"Fulano.Tal", "fulano.tal"}, {"\u212a", "k"}, {"\u017f", "S"}, {"ß", "SS"}, {"\u0130", "i"}, {"", ""},
		{"a", ""}, {"\u01c4", "\u01c6"}, {"\u01c5", "\u01c4"}, {"\u03c3", "\u03c2"}, {"\u03a3", "\u03c2"}, {"\ufb01", "fi"}, {"Á", "á"}, {"ÿ", "\u0178"},
		{"\u0131", "I"}, {"\u0131", "i"}, {"\u03a9", "\u2126"}, {"Å", "\u212b"}, {"SEDEMO/Ação", "sedemo/AÇÃO"}, {"localhost", "LOCALHOST"},
		{"e\u0301", "é"}, {"\u1e9e", "ß"}, {"\u1e9e", "ss"}, {"\u01c8", "\u01c7"}, {"açã\u212a", "AÇÃk"}, {"çã\u0131", "ÇÃI"},
	} {
		set.EqualFold = append(set.EqualFold, equalFoldCase{A: pair[0], B: pair[1], Equal: strings.EqualFold(pair[0], pair[1])})
	}
	for _, input := range []string{
		"a\u0085b", "a\u00a0b", "a\u3000b", "a\u200bb", "", "   ", " a  b ", "\ta\nb\r\vc\fd", "ção  é\u2028ü",
		"a\u1680b\u2000c\u200ad\u2029e\u202ff\u205fg", "a\u180eb", "a\ufeffb", "a\u001cb",
	} {
		set.Fields = append(set.Fields, fieldsCase{Input: input, Output: strings.Fields(input)})
	}
	for _, input := range []string{"\u0130", "\u0391\u03a3", "\u01c5", "\u01c4", "\u1e9e", "\u212a", "\u2126", "ÁÉÍÓÚÇ", "\u0130STANBUL", "\u039f\u0394\u039f\u03a3 \u03a3\u0391", "\u216b", "\uff21", "", "SEDEMO/NUPROC", "\U00010400"} {
		set.Lower = append(set.Lower, quoteCase{Input: input, Output: strings.ToLower(input)})
	}
	for _, input := range []string{
		"+3", "-0", "03", " 3", "3.0", "1_0", "9223372036854775808", "9223372036854775807", "-9223372036854775808",
		"-9223372036854775809", "", "+", "-", "0x10", "\u0663", "\uff13", "3 ", "++3", "+-3", "0", "60", "3650",
	} {
		c := atoiCase{Input: input}
		if value, err := strconv.Atoi(input); err == nil {
			c.Value = &value
		}
		set.Atoi = append(set.Atoi, c)
	}
	return set
}

type localTimeCase struct {
	Zone  string `json:"zone"`
	Local string `json:"local"`
	UTC   string `json:"utc"`
}

func localTimeVectors(t *testing.T) []localTimeCase {
	inputs := []struct {
		zone  string
		local string
	}{
		{"America/Sao_Paulo", "2018-11-03T23:59:59"},
		{"America/Sao_Paulo", "2018-11-04T00:00:00"},
		{"America/Sao_Paulo", "2018-11-04T00:30:00"},
		{"America/Sao_Paulo", "2018-11-04T00:59:59"},
		{"America/Sao_Paulo", "2018-11-04T01:00:00"},
		{"America/Sao_Paulo", "2018-11-04T23:59:59"},
		{"America/Sao_Paulo", "2019-02-16T22:59:59"},
		{"America/Sao_Paulo", "2019-02-16T23:00:00"},
		{"America/Sao_Paulo", "2019-02-16T23:30:00"},
		{"America/Sao_Paulo", "2019-02-16T23:59:59"},
		{"America/Sao_Paulo", "2019-02-17T00:00:00"},
		{"America/Sao_Paulo", "2026-11-10T00:00:00"},
		{"America/Sao_Paulo", "2026-11-10T23:59:59"},
		{"America/Boa_Vista", "2026-11-10T00:00:00"},
		{"America/Boa_Vista", "2026-11-10T23:59:59"},
		{"America/Boa_Vista", "2026-12-31T23:59:59"},
		{"America/Boa_Vista", "2018-11-04T00:30:00"},
		{"America/Boa_Vista", "2000-10-15T00:30:00"},
		{"America/Boa_Vista", "2000-10-15T00:00:00"},
		{"Brazil/East", "2018-11-04T00:30:00"},
		{"UTC", "2026-11-10T00:00:00"},
		{"UTC", "2026-11-10T23:59:59"},
		{"UTC", "2026-02-28T12:00:00"},
	}
	cases := make([]localTimeCase, len(inputs))
	for i, input := range inputs {
		location, err := time.LoadLocation(input.zone)
		if err != nil {
			t.Fatal(err)
		}
		parsed, err := time.Parse("2006-01-02T15:04:05", input.local)
		if err != nil {
			t.Fatal(err)
		}
		local := time.Date(parsed.Year(), parsed.Month(), parsed.Day(), parsed.Hour(), parsed.Minute(), parsed.Second(), 0, location)
		cases[i] = localTimeCase{Zone: input.zone, Local: input.local, UTC: local.UTC().Format(time.RFC3339Nano)}
	}
	return cases
}
