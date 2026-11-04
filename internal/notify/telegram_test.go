package notify_test

import (
	"encoding/json"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/notify"
)

const botToken = "123456789:AAE-test-token-0123456789abcdefghij"

type telegramPayload struct {
	ChatID      string `json:"chat_id"`
	Text        string `json:"text"`
	LinkPreview struct {
		Disabled bool `json:"is_disabled"`
	} `json:"link_preview_options"`
}

func newTelegram(api string) *notify.Telegram {
	return notify.NewTelegram(config.Telegram{Token: config.NewSecret(botToken), ChatID: "-1001234567890"}, api, "sei-monitor/test")
}

func TestTelegramSendsTheMessageToTheChat(t *testing.T) {
	server, rec := newServer(t, http.StatusOK, `{"ok":true,"result":{"message_id":1}}`)
	msg := notify.Compose([]event.Event{activity("00000.000001/2026-00", t0)}, event.Formatter{Location: time.UTC})

	err := newTelegram(server.URL).Send(t.Context(), msg)

	if err != nil {
		t.Fatal(err)
	}
	got := rec.all()[0]
	var payload telegramPayload
	if err := json.Unmarshal(got.body, &payload); err != nil {
		t.Fatal(err)
	}
	if got.method != http.MethodPost || got.path != "/bot"+botToken+"/sendMessage" || got.header.Get("Content-Type") != "application/json" {
		t.Errorf("request = %s %s %v", got.method, got.path, got.header)
	}
	if payload.ChatID != "-1001234567890" || payload.Text != "sei-monitor: 1 update on your processes\n\n"+msg.Body || !payload.LinkPreview.Disabled {
		t.Errorf("payload = %s", got.body)
	}
}

func TestTelegramCutsLongTexts(t *testing.T) {
	server, rec := newServer(t, http.StatusOK, `{"ok":true}`)

	err := newTelegram(server.URL).Send(t.Context(), notify.Message{Subject: "s", Body: strings.Repeat("Ofício ", 1000)})

	var payload telegramPayload
	if err != nil || json.Unmarshal(rec.all()[0].body, &payload) != nil || len(payload.Text) > 4000 || !strings.HasSuffix(payload.Text, "\n…") {
		t.Errorf("Send = %v, text of %d bytes", err, len(payload.Text))
	}
}

func TestTelegramErrorsNeverShowTheBotToken(t *testing.T) {
	refused, _ := newServer(t, http.StatusBadRequest, `{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}`)
	silent, _ := newServer(t, http.StatusBadGateway, "<html>bad gateway</html>")
	tests := map[string]struct {
		api  string
		want string
	}{
		"refused":     {refused.URL, "the server answered 400: Bad Request: chat not found"},
		"no reason":   {silent.URL, "the server answered 502"},
		"unreachable": {unreachableURL(t), "could not reach the server: "},
	}
	for name, tt := range tests {
		err := newTelegram(tt.api).Send(t.Context(), notify.NewTestMessage())
		if err == nil || !strings.HasPrefix(err.Error(), tt.want) || strings.Contains(err.Error(), "AAE-test-token") || strings.Contains(err.Error(), "/bot") {
			t.Errorf("%s: error = %v", name, err)
		}
	}
}
