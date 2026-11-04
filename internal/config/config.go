package config

import (
	"cmp"
	"errors"
	"fmt"
	"io/fs"
	"log/slog"
	"net"
	"net/mail"
	"net/url"
	"os"
	"os/exec"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"
	_ "time/tzdata"
)

const (
	DefaultDatabaseURL  = "sqlite:///data/sei-monitor.db"
	DefaultHTTPAddress  = ":8080"
	DefaultTimeZone     = "America/Sao_Paulo"
	DefaultPollInterval = 15 * time.Minute
	MinPollInterval     = 5 * time.Minute

	TLSStartTLS = "starttls"
	TLSImplicit = "tls"
	TLSNone     = "none"

	minTokenLength         = 32
	minWebhookSecretLength = 16
)

type Source struct {
	Lookup   func(string) (string, bool)
	ReadFile func(string) ([]byte, error)
}

func Environment() Source {
	return Source{Lookup: os.LookupEnv, ReadFile: os.ReadFile}
}

type Error struct {
	Problems []string
}

func (e *Error) Error() string {
	return "invalid configuration:\n  - " + strings.Join(e.Problems, "\n  - ")
}

type Config struct {
	SEI               SEI
	DatabaseURL       Secret
	PollInterval      time.Duration
	ReturnWarningDays int
	FullDetail        bool
	APIFullDetail     bool
	HTTP              HTTP
	Channels          Channels
	LogLevel          slog.Level
}

type SEI struct {
	URL         string
	APIURL      string
	Username    string
	Password    PasswordSource
	Org         string
	Units       []string
	AllUnits    bool
	Location    *time.Location
	Concurrency int
}

type HTTP struct {
	Address   string
	PublicURL string
	FeedToken Secret
	APIToken  Secret
}

func (h HTTP) Enabled() bool {
	return h.Address != ""
}

type Channels struct {
	Email    *Email
	Telegram *Telegram
	Ntfy     *Ntfy
	Webhook  *Webhook
	WhatsApp *WhatsApp
}

func (c Channels) Enabled() []string {
	channels := []struct {
		name string
		on   bool
	}{
		{"email", c.Email != nil},
		{"telegram", c.Telegram != nil},
		{"ntfy", c.Ntfy != nil},
		{"webhook", c.Webhook != nil},
		{"whatsapp", c.WhatsApp != nil},
	}
	var enabled []string
	for _, channel := range channels {
		if channel.on {
			enabled = append(enabled, channel.name)
		}
	}
	return enabled
}

type Email struct {
	Host     string
	Port     int
	Username string
	Password Secret
	From     string
	To       []string
	TLS      string
}

type Telegram struct {
	Token  Secret
	ChatID string
}

type Ntfy struct {
	URL   string
	Token Secret
}

type Webhook struct {
	URL    string
	Secret Secret
}

type WhatsApp struct {
	Recipients []string
}

func Load(src Source) (Config, error) {
	r := &reader{src: src}
	fullDetail := r.detailLevel("NOTIFICATION_DETAIL", false)
	cfg := Config{
		SEI:               r.sei(),
		DatabaseURL:       r.databaseURL(),
		PollInterval:      r.pollInterval(),
		ReturnWarningDays: r.integer("RETURN_WARNING_DAYS", 2, 0, 60),
		FullDetail:        fullDetail,
		APIFullDetail:     r.detailLevel("API_DETAIL", fullDetail),
		HTTP:              r.http(),
		Channels:          r.channels(),
		LogLevel:          r.logLevel(),
	}
	return cfg, r.err()
}

func LoadSEI(src Source) (SEI, error) {
	r := &reader{src: src}
	sei := r.sei()
	return sei, r.err()
}

func LoadSEIURL(src Source) (string, error) {
	r := &reader{src: src}
	seiURL := r.seiURL()
	return seiURL, r.err()
}

func LoadDatabaseURL(src Source) (Secret, error) {
	r := &reader{src: src}
	databaseURL := r.databaseURL()
	return databaseURL, r.err()
}

func LoadChannels(src Source) (Channels, error) {
	r := &reader{src: src}
	channels := r.channels()
	return channels, r.err()
}

type reader struct {
	src      Source
	problems []string
}

type urlRules struct {
	https bool
	query bool
}

func (r *reader) problem(format string, args ...any) {
	r.problems = append(r.problems, fmt.Sprintf(format, args...))
}

func (r *reader) err() error {
	if len(r.problems) == 0 {
		return nil
	}
	return &Error{Problems: r.problems}
}

func (r *reader) lookup(name string) (string, bool) {
	if r.src.Lookup == nil {
		return "", false
	}
	return r.src.Lookup(name)
}

func (r *reader) text(name string) string {
	value, _ := r.lookup(name)
	return strings.TrimSpace(value)
}

func (r *reader) required(name, hint string) string {
	value := r.text(name)
	if value == "" {
		r.problem("%s is required%s", name, hint)
	}
	return value
}

func (r *reader) secret(name string) Secret {
	direct, _ := r.lookup(name)
	if strings.TrimSpace(direct) == "" {
		direct = ""
	}
	path := r.text(name + "_FILE")
	if direct != "" && path != "" {
		r.problem("set either %s or %s_FILE, not both", name, name)
		return Secret{}
	}
	if path == "" {
		return NewSecret(direct)
	}
	return r.secretFile(name, path)
}

func (r *reader) secretFile(name, path string) Secret {
	readFile := r.src.ReadFile
	if readFile == nil {
		readFile = os.ReadFile
	}
	content, err := readFile(path)
	if err != nil {
		r.problem("%s_FILE: %s", name, describeFileError(path, err))
		return Secret{}
	}
	value := trimFinalNewline(string(content))
	if value == "" {
		r.problem("%s_FILE: %s is empty", name, path)
	}
	return NewSecret(value)
}

func describeFileError(path string, err error) string {
	reasons := []struct {
		target error
		format string
	}{
		{fs.ErrNotExist, "%s does not exist"},
		{fs.ErrPermission, "no permission to read %s"},
	}
	for _, reason := range reasons {
		if errors.Is(err, reason.target) {
			return fmt.Sprintf(reason.format, path)
		}
	}
	return fmt.Sprintf("could not read %s: %v", path, err)
}

func (r *reader) integer(name string, fallback, low, high int) int {
	value := r.text(name)
	if value == "" {
		return fallback
	}
	n, err := strconv.Atoi(value)
	if err != nil || n < low || n > high {
		r.problem("%s: use a whole number from %d to %d (got %q)", name, low, high, value)
		return fallback
	}
	return n
}

func (r *reader) parseURL(name, value string, rules urlRules) (*url.URL, bool) {
	parsed, err := url.Parse(value)
	if err != nil || parsed.Host == "" || (parsed.Scheme != "http" && parsed.Scheme != "https") {
		r.problem("%s: use a full address starting with https://", name)
		return nil, false
	}
	if parsed.User != nil {
		r.problem("%s: remove the username and password from the address", name)
		return nil, false
	}
	if parsed.Fragment != "" || (!rules.query && parsed.RawQuery != "") {
		r.problem("%s: remove everything after ? or #", name)
		return nil, false
	}
	if rules.https && parsed.Scheme != "https" && !isLoopback(parsed.Hostname()) {
		r.problem("%s: use https (plain http is only accepted for localhost)", name)
		return nil, false
	}
	return parsed, true
}

func isLoopback(host string) bool {
	if strings.EqualFold(host, "localhost") {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

func (r *reader) sei() SEI {
	sei := SEI{
		URL:         r.seiURL(),
		APIURL:      r.apiURL(),
		Username:    r.required("SEI_USERNAME", ""),
		Password:    r.seiPassword(),
		Org:         r.required("SEI_ORG", " (acronym or numeric id; the orgs subcommand lists them)"),
		Location:    r.location(),
		Concurrency: r.integer("SEI_CONCURRENCY", 3, 1, 10),
	}
	sei.Units, sei.AllUnits = parseUnits(r.text("SEI_UNITS"))
	return sei
}

func (r *reader) seiURL() string {
	value := r.required("SEI_URL", " (the SEI address, e.g. https://sei.rr.gov.br)")
	if value == "" {
		return ""
	}
	parsed, ok := r.parseURL("SEI_URL", value, urlRules{https: true})
	if !ok {
		return ""
	}
	parsed.Path = strings.TrimSuffix(strings.TrimRight(parsed.Path, "/"), "/sei")
	parsed.RawPath = ""
	return strings.TrimRight(parsed.String(), "/")
}

func (r *reader) apiURL() string {
	value := r.text("SEI_API_URL")
	if value == "" {
		return ""
	}
	parsed, ok := r.parseURL("SEI_API_URL", value, urlRules{https: true})
	if !ok {
		return ""
	}
	return strings.TrimRight(parsed.String(), "/")
}

func (r *reader) seiPassword() PasswordSource {
	direct, _ := r.lookup("SEI_PASSWORD")
	options := map[string]bool{
		"SEI_PASSWORD":      strings.TrimSpace(direct) != "",
		"SEI_PASSWORD_FILE": r.text("SEI_PASSWORD_FILE") != "",
		"SEI_PASSWORD_CMD":  r.text("SEI_PASSWORD_CMD") != "",
	}
	set := 0
	for _, present := range options {
		if present {
			set++
		}
	}
	if set == 0 {
		r.problem("SEI_PASSWORD is required (or SEI_PASSWORD_FILE, or SEI_PASSWORD_CMD)")
		return nil
	}
	if set > 1 {
		r.problem("set only one of SEI_PASSWORD, SEI_PASSWORD_FILE and SEI_PASSWORD_CMD")
		return nil
	}
	if !options["SEI_PASSWORD_CMD"] {
		return staticPassword{secret: r.secret("SEI_PASSWORD")}
	}
	command, err := NewPasswordCommand(r.text("SEI_PASSWORD_CMD"))
	if err != nil {
		r.problem("%v", err)
		return nil
	}
	if _, err := exec.LookPath(command.Program()); err != nil {
		r.problem("SEI_PASSWORD_CMD: program not found: %s", command.Program())
		return nil
	}
	return command
}

func (r *reader) location() *time.Location {
	name := cmp.Or(r.text("SEI_TIMEZONE"), DefaultTimeZone)
	location, err := time.LoadLocation(name)
	if err != nil {
		r.problem("SEI_TIMEZONE: unknown time zone %q (e.g. America/Boa_Vista)", name)
		return time.UTC
	}
	return location
}

func parseUnits(value string) ([]string, bool) {
	if strings.EqualFold(value, "all") {
		return nil, true
	}
	var units []string
	for part := range strings.SplitSeq(value, ",") {
		part = strings.TrimSpace(part)
		if part == "" || slices.ContainsFunc(units, func(unit string) bool { return strings.EqualFold(unit, part) }) {
			continue
		}
		units = append(units, part)
	}
	return units, false
}

func (r *reader) pollInterval() time.Duration {
	value := r.text("POLL_INTERVAL")
	if value == "" {
		return DefaultPollInterval
	}
	interval, err := time.ParseDuration(value)
	if err != nil {
		r.problem("POLL_INTERVAL: use a duration such as 15m or 1h (got %q)", value)
		return DefaultPollInterval
	}
	if interval < MinPollInterval {
		r.problem("POLL_INTERVAL: the minimum is %s, to keep the load on the agency's SEI low (got %s)", MinPollInterval, value)
		return MinPollInterval
	}
	return interval
}

func (r *reader) databaseURL() Secret {
	value := r.secret("DATABASE_URL")
	if value.IsZero() {
		return NewSecret(DefaultDatabaseURL)
	}
	schemes := []string{"sqlite://", "postgres://", "postgresql://"}
	valid := slices.ContainsFunc(schemes, func(scheme string) bool {
		rest, found := strings.CutPrefix(value.Reveal(), scheme)
		return found && rest != ""
	})
	if !valid {
		r.problem("DATABASE_URL: use sqlite:///path/to/file.db or postgres://user:password@host/database")
	}
	return value
}

func (r *reader) http() HTTP {
	address, set := r.lookup("HTTP_ADDRESS")
	address = strings.TrimSpace(address)
	if !set {
		address = DefaultHTTPAddress
	}
	if address == "" {
		return HTTP{}
	}
	if _, port, err := net.SplitHostPort(address); err != nil || !validPort(port) {
		r.problem("HTTP_ADDRESS: use host:port or :port, such as :8080 (got %q)", address)
	}
	feed, api := r.secret("FEED_TOKEN"), r.secret("API_TOKEN")
	r.checkToken("FEED_TOKEN", feed)
	r.checkToken("API_TOKEN", api)
	if feed.IsZero() && api.IsZero() {
		r.problem("with the HTTP server on, set FEED_TOKEN, API_TOKEN or both (create them with the generate-token command), or leave HTTP_ADDRESS empty")
	}
	if !feed.IsZero() && feed.Reveal() == api.Reveal() {
		r.problem("API_TOKEN must be different from FEED_TOKEN")
	}
	return HTTP{Address: address, PublicURL: r.publicURL(), FeedToken: feed, APIToken: api}
}

func (r *reader) publicURL() string {
	value := r.text("PUBLIC_URL")
	if value == "" {
		return ""
	}
	parsed, ok := r.parseURL("PUBLIC_URL", value, urlRules{https: true})
	if !ok {
		return ""
	}
	return strings.TrimRight(parsed.String(), "/")
}

func validPort(port string) bool {
	n, err := strconv.Atoi(port)
	return err == nil && n >= 1 && n <= 65535
}

func (r *reader) checkToken(name string, token Secret) {
	if token.IsZero() {
		return
	}
	if len(token.Reveal()) < minTokenLength {
		r.problem("%s: use at least %d characters (create one with the generate-token command)", name, minTokenLength)
	}
	if !regexp.MustCompile(`^[A-Za-z0-9._~-]+$`).MatchString(token.Reveal()) {
		r.problem("%s: use only letters, digits, '-', '_', '.' and '~'", name)
	}
}

func (r *reader) detailLevel(name string, fallback bool) bool {
	value := strings.ToLower(r.text(name))
	if value == "" {
		return fallback
	}
	levels := map[string]bool{"minimal": false, "full": true}
	full, ok := levels[value]
	if !ok {
		r.problem("%s: use minimal or full (got %q)", name, value)
		return fallback
	}
	return full
}

func (r *reader) logLevel() slog.Level {
	value := cmp.Or(r.text("LOG_LEVEL"), "info")
	var level slog.Level
	if err := level.UnmarshalText([]byte(value)); err != nil {
		r.problem("LOG_LEVEL: use debug, info, warn or error (got %q)", value)
		return slog.LevelInfo
	}
	return level
}

func (r *reader) channels() Channels {
	return Channels{
		Email:    r.email(),
		Telegram: r.telegram(),
		Ntfy:     r.ntfy(),
		Webhook:  r.webhook(),
		WhatsApp: r.whatsapp(),
	}
}

func (r *reader) email() *Email {
	host := r.text("SMTP_HOST")
	if host == "" {
		return nil
	}
	mode := strings.ToLower(cmp.Or(r.text("SMTP_TLS"), TLSStartTLS))
	ports := map[string]int{TLSStartTLS: 587, TLSImplicit: 465, TLSNone: 25}
	port, ok := ports[mode]
	if !ok {
		r.problem("SMTP_TLS: use starttls, tls or none (got %q)", mode)
	}
	email := &Email{
		Host:     host,
		Port:     r.integer("SMTP_PORT", port, 1, 65535),
		Username: r.text("SMTP_USERNAME"),
		Password: r.secret("SMTP_PASSWORD"),
		From:     r.address("SMTP_FROM"),
		To:       r.addresses("SMTP_TO"),
		TLS:      mode,
	}
	if (email.Username == "") != email.Password.IsZero() {
		r.problem("SMTP_USERNAME and SMTP_PASSWORD go together: set both or neither")
	}
	if mode == TLSNone && !email.Password.IsZero() {
		r.problem("SMTP_TLS=none would send the password unencrypted; use starttls or tls")
	}
	return email
}

func (r *reader) address(name string) string {
	value := r.required(name, "")
	if value == "" {
		return ""
	}
	if _, err := mail.ParseAddress(value); err != nil {
		r.problem("%s: invalid email address %q", name, value)
		return ""
	}
	return value
}

func (r *reader) addresses(name string) []string {
	value := r.required(name, "")
	if value == "" {
		return nil
	}
	list, err := mail.ParseAddressList(value)
	if err != nil {
		r.problem("%s: invalid email address list %q", name, value)
		return nil
	}
	addresses := make([]string, 0, len(list))
	for _, address := range list {
		addresses = append(addresses, address.Address)
	}
	return addresses
}

func (r *reader) telegram() *Telegram {
	token := r.secret("TELEGRAM_TOKEN")
	if token.IsZero() {
		return nil
	}
	if !regexp.MustCompile(`^\d+:[A-Za-z0-9_-]{20,}$`).MatchString(token.Reveal()) {
		r.problem("TELEGRAM_TOKEN: this does not look like a bot token (format 123456789:AAE...)")
	}
	chatID := r.required("TELEGRAM_CHAT_ID", " (numeric chat id or @channel)")
	if chatID != "" && !regexp.MustCompile(`^(-?\d+|@[A-Za-z][A-Za-z0-9_]{3,})$`).MatchString(chatID) {
		r.problem("TELEGRAM_CHAT_ID: use the numeric chat id (e.g. -1001234567890) or @channel_name")
	}
	return &Telegram{Token: token, ChatID: chatID}
}

func (r *reader) ntfy() *Ntfy {
	value := r.text("NTFY_URL")
	if value == "" {
		return nil
	}
	parsed, ok := r.parseURL("NTFY_URL", value, urlRules{})
	if ok && strings.Trim(parsed.Path, "/") == "" {
		r.problem("NTFY_URL: end the address with the topic (e.g. https://ntfy.sh/my-topic)")
	}
	return &Ntfy{URL: strings.TrimRight(value, "/"), Token: r.secret("NTFY_TOKEN")}
}

func (r *reader) webhook() *Webhook {
	value := r.text("WEBHOOK_URL")
	if value == "" {
		return nil
	}
	r.parseURL("WEBHOOK_URL", value, urlRules{query: true})
	secret := r.secret("WEBHOOK_SECRET")
	if secret.IsZero() {
		r.problem("WEBHOOK_SECRET is required; it signs the webhook body")
	} else if len(secret.Reveal()) < minWebhookSecretLength {
		r.problem("WEBHOOK_SECRET: use at least %d characters", minWebhookSecretLength)
	}
	return &Webhook{URL: value, Secret: secret}
}

func (r *reader) whatsapp() *WhatsApp {
	value := r.text("WHATSAPP_RECIPIENTS")
	if value == "" {
		return nil
	}
	whatsapp := &WhatsApp{}
	for item := range strings.SplitSeq(value, ",") {
		item = strings.TrimSpace(item)
		if item == "" {
			continue
		}
		recipient, ok := whatsappRecipient(item)
		if !ok {
			r.problem("WHATSAPP_RECIPIENTS: %q is neither a phone number with country and area code (e.g. 5595991234567) nor a group id (ending in @g.us)", item)
			continue
		}
		whatsapp.Recipients = append(whatsapp.Recipients, recipient)
	}
	return whatsapp
}

func whatsappRecipient(item string) (string, bool) {
	if strings.HasSuffix(item, "@g.us") {
		return item, regexp.MustCompile(`^\d+(-\d+)?@g\.us$`).MatchString(item)
	}
	if !regexp.MustCompile(`^\+?[\d\s().-]+$`).MatchString(item) {
		return "", false
	}
	digits := strings.Map(func(r rune) rune {
		if r >= '0' && r <= '9' {
			return r
		}
		return -1
	}, item)
	return digits, len(digits) >= 10 && len(digits) <= 15
}
