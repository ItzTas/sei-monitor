package notify_test

import (
	"net/http"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/notify"
)

func TestNtfyPostsTheTextWithTitleTagsAndPriority(t *testing.T) {
	tests := []struct {
		name          string
		events        []event.Event
		token         string
		priority      string
		authorization string
	}{
		{"routine, public topic", []event.Event{activity("00000.000001/2026-00", t0)}, "", "default", ""},
		{"urgent, protected topic", []event.Event{{Kind: event.ReturnOverdue, Payload: event.Payload{ProcessNumber: "00000.000002/2026-00"}}}, "tk_0123456789", "high", "Bearer tk_0123456789"},
	}
	for _, tt := range tests {
		server, rec := newServer(t, http.StatusOK, `{"id":"abc"}`)
		msg := notify.Compose(tt.events, event.Formatter{Location: time.UTC})

		err := notify.NewNtfy(config.Ntfy{URL: server.URL + "/sei-topic", Token: config.NewSecret(tt.token)}, "sei-monitor/test").Send(t.Context(), msg)

		if err != nil {
			t.Fatalf("%s: Send = %v", tt.name, err)
		}
		got := rec.all()[0]
		header := got.header
		if got.path != "/sei-topic" || string(got.body) != msg.Body || header.Get("Title") != "sei-monitor: 1 update on your processes" ||
			header.Get("Tags") != "sei-monitor" || header.Get("Priority") != tt.priority || header.Get("Authorization") != tt.authorization {
			t.Errorf("%s: request %s %v %q", tt.name, got.path, header, got.body)
		}
	}
}

func TestNtfyCutsLongTextsAtACharacter(t *testing.T) {
	server, rec := newServer(t, http.StatusOK, "")

	err := notify.NewNtfy(config.Ntfy{URL: server.URL + "/sei-topic"}, "sei-monitor/test").Send(t.Context(), notify.Message{Subject: "s", Body: strings.Repeat("Ofício ", 1000)})

	body := string(rec.all()[0].body)
	if err != nil || len(body) > 4000 || !utf8.ValidString(body) || !strings.HasSuffix(body, "\n…") || !strings.HasPrefix(body, "Ofício Ofício") {
		t.Errorf("Send = %v, %d bytes, ends with %q", err, len(body), body[max(0, len(body)-12):])
	}
}
