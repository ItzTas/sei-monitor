package notify_test

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/notify"
)

const webhookSecret = "0123456789abcdef0123456789abcdef"

func newWebhook(url string) *notify.Webhook {
	return notify.NewWebhook(config.Webhook{URL: url, Secret: config.NewSecret(webhookSecret)}, "sei-monitor/test")
}

func TestWebhookPostsTheEventsWithAnHMACSignature(t *testing.T) {
	server, rec := newServer(t, http.StatusNoContent, "")
	msg := notify.Compose([]event.Event{activity("00000.000001/2026-00", t0)}, event.Formatter{Location: time.UTC})

	err := newWebhook(server.URL+"/sei?key=abc").Send(t.Context(), msg)

	requests := rec.all()
	if err != nil || len(requests) != 1 {
		t.Fatalf("Send = %v, %d requests", err, len(requests))
	}
	got := requests[0]
	mac := hmac.New(sha256.New, []byte(webhookSecret))
	mac.Write(got.body)
	if got.header.Get(notify.SignatureHeader) != "sha256="+hex.EncodeToString(mac.Sum(nil)) {
		t.Errorf("signature = %q", got.header.Get(notify.SignatureHeader))
	}
	if got.method != http.MethodPost || got.path != "/sei" || got.header.Get("Content-Type") != "application/json" || got.header.Get("User-Agent") != "sei-monitor/test" {
		t.Errorf("request = %s %s %v", got.method, got.path, got.header)
	}
	var payload struct {
		Test   *bool        `json:"test"`
		Events []event.View `json:"events"`
	}
	if err := json.Unmarshal(got.body, &payload); err != nil {
		t.Fatal(err)
	}
	if payload.Test != nil || len(payload.Events) != 1 || payload.Events[0].Kind != event.ActivityAdded || payload.Events[0].Title != msg.Events[0].Title {
		t.Errorf("payload = %s", got.body)
	}
}

func TestWebhookMarksTheTestMessage(t *testing.T) {
	server, rec := newServer(t, http.StatusOK, "")

	err := newWebhook(server.URL).Send(t.Context(), notify.NewTestMessage())

	if err != nil || string(rec.all()[0].body) != `{"test":true,"events":[]}` {
		t.Errorf("Send = %v, body %s", err, rec.all()[0].body)
	}
}

func TestWebhookErrorsLeaveTheAddressOut(t *testing.T) {
	target, targetRequests := newServer(t, http.StatusOK, "")
	failing, _ := newServer(t, http.StatusInternalServerError, "internal error: key=s3cr3t")
	redirecting, _ := newServer(t, http.StatusFound, "", "Location", target.URL+"/elsewhere")
	tests := map[string]struct {
		url  string
		want string
	}{
		"server error": {failing.URL, "the server answered 500"},
		"redirect":     {redirecting.URL, "the server answered 302"},
		"unreachable":  {unreachableURL(t), "could not reach the server: "},
	}
	for name, tt := range tests {
		err := newWebhook(tt.url+"/hook/s3cr3t?key=s3cr3t").Send(t.Context(), notify.NewTestMessage())
		if err == nil || !strings.HasPrefix(err.Error(), tt.want) || strings.Contains(err.Error(), "s3cr3t") || strings.Contains(err.Error(), "/hook") {
			t.Errorf("%s: error = %v", name, err)
		}
	}
	if requests := targetRequests.all(); len(requests) != 0 {
		t.Errorf("the client followed the redirect: %+v", requests)
	}
}
