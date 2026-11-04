package notify

import (
	"context"
	"mime"
	"net/http"
	"strings"

	"github.com/ItzTas/sei-monitor/internal/config"
)

const ntfyMaxBytes = 4000

type Ntfy struct {
	url       string
	token     config.Secret
	userAgent string
	client    *http.Client
}

func NewNtfy(cfg config.Ntfy, userAgent string) *Ntfy {
	return &Ntfy{url: cfg.URL, token: cfg.Token, userAgent: userAgent, client: newHTTPClient()}
}

func (n *Ntfy) Name() string {
	return "ntfy"
}

func (n *Ntfy) Send(ctx context.Context, msg Message) error {
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, n.url, strings.NewReader(cut(msg.Body, ntfyMaxBytes)))
	if err != nil {
		return errBadRequest
	}
	request.Header.Set("Content-Type", "text/plain; charset=utf-8")
	request.Header.Set("User-Agent", n.userAgent)
	request.Header.Set("Title", mime.QEncoding.Encode("utf-8", "sei-monitor: "+msg.Subject))
	request.Header.Set("Tags", "sei-monitor")
	request.Header.Set("Priority", map[bool]string{false: "default", true: "high"}[msg.Urgent])
	if !n.token.IsZero() {
		request.Header.Set("Authorization", "Bearer "+n.token.Reveal())
	}
	status, _, err := post(n.client, request)
	if err != nil {
		return err
	}
	return expectSuccess(status)
}
