package sei

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/ItzTas/sei-monitor/internal/config"
)

const (
	routeLogin      = "/autenticar"
	routeVersion    = "/versao"
	routeProcesses  = "/processo/listar"
	routeActivities = "/atividade/listar"
	routeDocuments  = "/documento/listar"

	ProcessPageSize = 50
	requestTimeout  = 30 * time.Second
	maxBodySize     = 16 << 20
	maxPages        = 400
)

type Options struct {
	UserAgent string
	Location  *time.Location
	Transport http.RoundTripper
}

func (o Options) apiClient() *http.Client {
	return &http.Client{
		Timeout:   requestTimeout,
		Transport: o.Transport,
		CheckRedirect: func(*http.Request, []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}
}

func (o Options) pageClient() *http.Client {
	return &http.Client{Timeout: requestTimeout, Transport: o.Transport}
}

func (o Options) location() *time.Location {
	if o.Location == nil {
		return time.UTC
	}
	return o.Location
}

type Credentials struct {
	Username string
	Password config.PasswordSource
	OrgID    int64
}

type Client struct {
	base      string
	creds     Credentials
	userAgent string
	location  *time.Location
	http      *http.Client

	mu         sync.Mutex
	token      config.Secret
	generation uint64
	session    Session
}

type request struct {
	method string
	route  string
	path   string
	query  url.Values
	form   url.Values
	unit   int64
}

type RawPage struct {
	Data  json.RawMessage
	Total json.RawMessage
}

func New(apiBase string, creds Credentials, opts Options) *Client {
	return &Client{
		base:      strings.TrimRight(apiBase, "/"),
		creds:     creds,
		userAgent: opts.UserAgent,
		location:  opts.location(),
		http:      opts.apiClient(),
	}
}

func (c *Client) BaseURL() string {
	return c.base
}

func (c *Client) Login(ctx context.Context) (Session, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if err := c.loginLocked(ctx); err != nil {
		return Session{}, err
	}
	return c.session, nil
}

func (c *Client) Session(ctx context.Context) (Session, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if !c.token.IsZero() {
		return c.session, nil
	}
	if err := c.loginLocked(ctx); err != nil {
		return Session{}, err
	}
	return c.session, nil
}

func (c *Client) Version(ctx context.Context) (Version, error) {
	env, err := c.authorized(ctx, request{method: http.MethodGet, route: routeVersion, path: routeVersion})
	if err != nil {
		return Version{}, err
	}
	var wire versionWire
	if err := json.Unmarshal(env.Data, &wire); err != nil {
		return Version{}, fmt.Errorf("sei: read the %s answer: %w", routeVersion, err)
	}
	return Version(wire), nil
}

func (c *Client) Processes(ctx context.Context, unitID int64) ([]Process, error) {
	var all []Process
	seen := map[int64]bool{}
	for page := range maxPages {
		result, err := c.ProcessPage(ctx, unitID, ProcessQuery{Page: page, Limit: ProcessPageSize})
		if err != nil {
			return nil, err
		}
		added := 0
		for _, process := range result.Items {
			if process.ProcedureID == 0 || seen[process.ProcedureID] {
				continue
			}
			seen[process.ProcedureID] = true
			all = append(all, process)
			added++
		}
		if added == 0 || len(result.Items) < ProcessPageSize || (result.Total > 0 && len(all) >= result.Total) {
			return all, nil
		}
	}
	return nil, fmt.Errorf("sei: the process list went past %d pages", maxPages)
}

func (c *Client) ProcessPage(ctx context.Context, unitID int64, query ProcessQuery) (Page[Process], error) {
	values := pageQuery(query.Page, query.Limit)
	if query.Type != "" {
		values.Set("tipo", query.Type)
	}
	req := request{method: http.MethodGet, route: routeProcesses, path: routeProcesses, query: values, unit: unitID}
	return fetchPage(ctx, c, req, processWire.process)
}

func (c *Client) ActivityPage(ctx context.Context, unitID, procedureID int64, page, limit int) (Page[Activity], error) {
	values := pageQuery(page, limit)
	values.Set("procedimento", strconv.FormatInt(procedureID, 10))
	req := request{method: http.MethodGet, route: routeActivities, path: routeActivities, query: values, unit: unitID}
	return fetchPage(ctx, c, req, func(wire activityWire) Activity {
		return wire.activity(procedureID, c.location)
	})
}

func (c *Client) DocumentPage(ctx context.Context, unitID, procedureID int64, page, limit int) (Page[Document], error) {
	path := routeDocuments + "/" + strconv.FormatInt(procedureID, 10)
	req := request{method: http.MethodGet, route: routeDocuments, path: path, query: pageQuery(page, limit), unit: unitID}
	return fetchPage(ctx, c, req, func(wire documentWire) Document {
		return wire.document(procedureID)
	})
}

func (c *Client) Raw(ctx context.Context, unitID int64, path string, query url.Values) (RawPage, error) {
	env, err := c.authorized(ctx, request{method: http.MethodGet, route: path, path: path, query: query, unit: unitID})
	if err != nil {
		return RawPage{}, err
	}
	return RawPage{Data: env.Data, Total: env.Total}, nil
}

func pageQuery(page, limit int) url.Values {
	return url.Values{"limit": {strconv.Itoa(limit)}, "start": {strconv.Itoa(page)}}
}

func fetchPage[W, T any](ctx context.Context, c *Client, req request, convert func(W) T) (Page[T], error) {
	env, err := c.authorized(ctx, req)
	if err != nil {
		return Page[T]{}, err
	}
	var wires []W
	if err := env.list(&wires); err != nil {
		return Page[T]{}, fmt.Errorf("sei: read the %s answer: %w", req.route, err)
	}
	page := Page[T]{Items: make([]T, 0, len(wires)), Total: env.total()}
	for _, wire := range wires {
		page.Items = append(page.Items, convert(wire))
	}
	return page, nil
}

func (c *Client) authorized(ctx context.Context, req request) (*envelope, error) {
	token, generation, err := c.currentToken(ctx)
	if err != nil {
		return nil, err
	}
	env, err := c.send(ctx, req, token)
	if !errors.Is(err, ErrUnauthorized) {
		return env, err
	}
	token, err = c.renewToken(ctx, generation)
	if err != nil {
		return nil, err
	}
	return c.send(ctx, req, token)
}

func (c *Client) currentToken(ctx context.Context) (config.Secret, uint64, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.token.IsZero() {
		if err := c.loginLocked(ctx); err != nil {
			return config.Secret{}, 0, err
		}
	}
	return c.token, c.generation, nil
}

func (c *Client) renewToken(ctx context.Context, seen uint64) (config.Secret, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.generation != seen && !c.token.IsZero() {
		return c.token, nil
	}
	if err := c.loginLocked(ctx); err != nil {
		return config.Secret{}, err
	}
	return c.token, nil
}

func (c *Client) loginLocked(ctx context.Context) error {
	c.token = config.Secret{}
	env, err := c.sendLogin(ctx)
	if err != nil {
		return err
	}
	var wire loginWire
	if err := json.Unmarshal(env.Data, &wire); err != nil {
		return fmt.Errorf("sei: read the %s answer: %w", routeLogin, err)
	}
	if wire.Token == "" {
		return fmt.Errorf("sei: %s: SEI did not return a session token", routeLogin)
	}
	c.token = config.NewSecret(wire.Token)
	c.generation++
	c.session = wire.session()
	return nil
}

func (c *Client) sendLogin(ctx context.Context) (*envelope, error) {
	password, err := c.creds.Password.Password(ctx)
	if err != nil {
		return nil, fmt.Errorf("sei: %s: %w", routeLogin, err)
	}
	form := url.Values{
		"usuario":  {c.creds.Username},
		"senha":    {password.Reveal()},
		"orgao":    {strconv.FormatInt(c.creds.OrgID, 10)},
		"contexto": {""},
	}
	return c.send(ctx, request{method: http.MethodPost, route: routeLogin, path: routeLogin, form: form}, config.Secret{})
}

func (c *Client) send(ctx context.Context, req request, token config.Secret) (*envelope, error) {
	target := c.base + req.path
	if len(req.query) > 0 {
		target += "?" + req.query.Encode()
	}
	var body io.Reader
	if req.form != nil {
		body = strings.NewReader(req.form.Encode())
	}
	httpReq, err := http.NewRequestWithContext(ctx, req.method, target, body)
	if err != nil {
		return nil, fmt.Errorf("sei: build the %s call: %w", req.route, withoutQuery(err))
	}
	httpReq.Header.Set("Accept", "application/json")
	httpReq.Header.Set("User-Agent", c.userAgent)
	if req.form != nil {
		httpReq.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	}
	if !token.IsZero() {
		httpReq.Header.Set("token", token.Reveal())
	}
	if req.unit != 0 {
		httpReq.Header.Set("unidade", strconv.FormatInt(req.unit, 10))
	}
	resp, err := c.http.Do(httpReq)
	if err != nil {
		return nil, fmt.Errorf("sei: %s: %w", req.route, withoutQuery(err))
	}
	defer func() { _ = resp.Body.Close() }()
	return readEnvelope(req.route, resp)
}

func readEnvelope(route string, resp *http.Response) (*envelope, error) {
	if resp.StatusCode >= http.StatusMultipleChoices && resp.StatusCode < http.StatusBadRequest {
		message := "SEI answered with a redirect, which sei-monitor does not follow on API calls; check SEI_URL and SEI_API_URL"
		return nil, &APIError{Route: route, Status: resp.StatusCode, Message: message, kind: ErrRedirect}
	}
	env, ok, err := decode(resp)
	if err != nil {
		return nil, fmt.Errorf("sei: read the %s answer: %w", route, err)
	}
	if !ok {
		return nil, unexpected(route, resp.StatusCode)
	}
	if *env.Success && resp.StatusCode < http.StatusMultipleChoices {
		return &env, nil
	}
	return nil, newAPIError(route, resp.StatusCode, env.message())
}

func decode(resp *http.Response) (envelope, bool, error) {
	body, err := io.ReadAll(io.LimitReader(resp.Body, maxBodySize))
	if err != nil {
		return envelope{}, false, err
	}
	var env envelope
	if json.Unmarshal(body, &env) != nil || env.Success == nil {
		return envelope{}, false, nil
	}
	return env, true, nil
}

func unexpected(route string, status int) *APIError {
	kind := ErrUnexpectedResponse
	if status >= http.StatusInternalServerError {
		kind = ErrServer
	}
	return &APIError{Route: route, Status: status, Message: fmt.Sprintf("answer outside the API format (HTTP %d)", status), kind: kind}
}
