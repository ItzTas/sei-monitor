package notify_test

import (
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/notify"
	"github.com/ItzTas/sei-monitor/internal/notify/notifytest"
)

func emailConfig(server *notifytest.SMTPServer, security string) core.Email {
	return core.Email{
		Host: server.Host, Port: server.Port, TLS: security,
		From: "sei-monitor@example.org", To: []string{"fulano@example.org", "beltrano@example.org"},
	}
}

func TestEmailOverStartTLSWithLogin(t *testing.T) {
	server := notifytest.NewSMTPServer(t, notifytest.SMTPStartTLS)
	cfg := emailConfig(server, core.TLSStartTLS)
	cfg.Username, cfg.Password = "fulano", core.NewSecret("senha-smtp")
	events := []event.Event{
		activity("00000.000001/2026-00", t0),
		{Kind: event.DocumentAdded, Payload: event.Payload{ProcessNumber: "00000.000002/2026-00"}},
	}
	msg := notify.Compose(events, event.Formatter{Location: time.UTC})

	err := notify.NewEmail(cfg, "sei-monitor/test", server.ClientTLS).Send(t.Context(), msg)

	mails := server.Mails()
	if err != nil || len(mails) != 1 {
		t.Fatalf("Send = %v, %d mails", err, len(mails))
	}
	header, text, err := mails[0].Parse()
	if err != nil {
		t.Fatal(err)
	}
	if mails[0].From != cfg.From || !slices.Equal(mails[0].To, cfg.To) || header.Get("Subject") != "[sei-monitor] 2 updates on your processes" || strings.TrimSpace(text) != msg.Body {
		t.Errorf("mail from %s to %v, subject %q, text %q", mails[0].From, mails[0].To, header.Get("Subject"), text)
	}
	if logins := server.Logins(); !slices.Equal(logins, []notifytest.Login{{Username: "fulano", Password: "senha-smtp"}}) {
		t.Errorf("logins = %+v", logins)
	}
}

func TestEmailOverImplicitTLSAndWithoutTLS(t *testing.T) {
	tests := map[string]struct {
		mode     notifytest.SMTPMode
		security string
	}{
		"implicit TLS": {notifytest.SMTPImplicitTLS, core.TLSImplicit},
		"no TLS":       {notifytest.SMTPPlain, core.TLSNone},
	}
	for name, tt := range tests {
		server := notifytest.NewSMTPServer(t, tt.mode)

		err := notify.NewEmail(emailConfig(server, tt.security), "sei-monitor/test", server.ClientTLS).Send(t.Context(), notify.NewTestMessage())

		mails := server.Mails()
		if err != nil || len(mails) != 1 {
			t.Fatalf("%s: Send = %v, %d mails", name, err, len(mails))
		}
		if header, _, err := mails[0].Parse(); err != nil || header.Get("Subject") != "[sei-monitor] test message" {
			t.Errorf("%s: subject %q (%v)", name, header.Get("Subject"), err)
		}
	}
}

func TestEmailRefusesAServerWithoutStartTLS(t *testing.T) {
	server := notifytest.NewSMTPServer(t, notifytest.SMTPPlain)
	cfg := emailConfig(server, core.TLSStartTLS)
	cfg.Username, cfg.Password = "fulano", core.NewSecret("senha-smtp")

	err := notify.NewEmail(cfg, "sei-monitor/test", server.ClientTLS).Send(t.Context(), notify.NewTestMessage())

	if err == nil || len(server.Mails()) != 0 || len(server.Logins()) != 0 || strings.Contains(err.Error(), "senha-smtp") {
		t.Errorf("Send = %v, mails %d, logins %d", err, len(server.Mails()), len(server.Logins()))
	}
}
