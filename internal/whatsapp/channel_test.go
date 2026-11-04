package whatsapp_test

import (
	"context"
	"errors"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/types"

	"github.com/ItzTas/sei-monitor/internal/notify"
	"github.com/ItzTas/sei-monitor/internal/whatsapp"
)

const group = "120363012345678901@g.us"

var registered = types.NewJID("559591234567", types.DefaultUserServer)

type delivered struct {
	to   types.JID
	text string
}

type fakeMessenger struct {
	mu         sync.Mutex
	connected  bool
	loggedIn   bool
	connectErr error
	connects   int
	numbers    map[string]types.JID
	lookups    int
	sent       []delivered
}

func (f *fakeMessenger) IsConnected() bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.connected
}

func (f *fakeMessenger) ConnectContext(context.Context) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.connects++
	if f.connectErr != nil {
		return f.connectErr
	}
	f.connected = true
	return nil
}

func (f *fakeMessenger) WaitForConnection(time.Duration) bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.connected && f.loggedIn
}

func (f *fakeMessenger) IsOnWhatsApp(_ context.Context, phones []string) ([]types.IsOnWhatsAppResponse, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.lookups++
	answers := make([]types.IsOnWhatsAppResponse, 0, len(phones))
	for _, phone := range phones {
		jid, ok := f.numbers[phone]
		answers = append(answers, types.IsOnWhatsAppResponse{Query: phone, JID: jid, IsIn: ok})
	}
	return answers, nil
}

func (f *fakeMessenger) SendMessage(_ context.Context, to types.JID, message *waE2E.Message, _ ...whatsmeow.SendRequestExtra) (whatsmeow.SendResponse, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.sent = append(f.sent, delivered{to: to, text: message.GetConversation()})
	return whatsmeow.SendResponse{}, nil
}

func online() *fakeMessenger {
	return &fakeMessenger{connected: true, loggedIn: true, numbers: map[string]types.JID{"+5595991234567": registered}}
}

func TestSendsToTheNumberWhatsAppKnowsAndToGroups(t *testing.T) {
	messenger := online()
	channel := whatsapp.NewChannel(messenger, []string{"5595991234567", group})
	msg := notify.Message{Subject: "1 update on your processes", Body: "New document on process 00000.000001/2026-00"}

	first := channel.Send(t.Context(), msg)
	second := channel.Send(t.Context(), msg)

	if first != nil || second != nil {
		t.Fatalf("Send = %v, %v", first, second)
	}
	text := "sei-monitor: 1 update on your processes\n\nNew document on process 00000.000001/2026-00"
	groupJID, _ := types.ParseJID(group)
	want := []delivered{{registered, text}, {groupJID, text}, {registered, text}, {groupJID, text}}
	if !slices.Equal(messenger.sent, want) || messenger.lookups != 1 || messenger.connects != 0 {
		t.Errorf("sent %+v, %d lookups, %d connects", messenger.sent, messenger.lookups, messenger.connects)
	}
}

func TestANumberOffWhatsAppDoesNotStopTheOtherRecipients(t *testing.T) {
	messenger := online()
	channel := whatsapp.NewChannel(messenger, []string{"5595990000000", group})

	err := channel.Send(t.Context(), notify.NewTestMessage())

	if err == nil || err.Error() != "recipient 1 of 2: the number is not on WhatsApp" || len(messenger.sent) != 1 || messenger.sent[0].to.Server != types.GroupServer {
		t.Errorf("Send = %v, sent %+v", err, messenger.sent)
	}
	if strings.Contains(err.Error(), "5595990000000") {
		t.Errorf("the error shows the number: %v", err)
	}
}

func TestConnectsOnDemand(t *testing.T) {
	tests := map[string]struct {
		messenger *fakeMessenger
		want      string
	}{
		"reconnects":      {&fakeMessenger{loggedIn: true, numbers: map[string]types.JID{"+5595991234567": registered}}, ""},
		"not paired":      {&fakeMessenger{connectErr: whatsapp.ErrNotPaired}, "could not connect to WhatsApp: not paired with WhatsApp; run sei-monitor whatsapp-pair"},
		"login times out": {&fakeMessenger{}, "the WhatsApp session did not log in within 30s"},
	}
	for name, tt := range tests {
		err := whatsapp.NewChannel(tt.messenger, []string{"5595991234567"}).Send(t.Context(), notify.NewTestMessage())

		got := ""
		if err != nil {
			got = err.Error()
		}
		if got != tt.want || tt.messenger.connects != 1 {
			t.Errorf("%s: Send = %q, %d connects", name, got, tt.messenger.connects)
		}
		if tt.want != "" && len(tt.messenger.sent) != 0 {
			t.Errorf("%s: sent %+v", name, tt.messenger.sent)
		}
	}
	if !errors.Is(whatsapp.NewChannel(&fakeMessenger{connectErr: whatsapp.ErrNotPaired}, nil).Send(t.Context(), notify.NewTestMessage()), whatsapp.ErrNotPaired) {
		t.Error("the not paired error does not unwrap to ErrNotPaired")
	}
}
