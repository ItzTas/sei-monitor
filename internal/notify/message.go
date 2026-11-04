package notify

import (
	"fmt"
	"strings"
	"unicode/utf8"

	"github.com/ItzTas/sei-monitor/internal/event"
)

const MaxListed = 10

type Message struct {
	Subject string
	Body    string
	Events  []event.View
	Urgent  bool
	Test    bool
}

func Compose(events []event.Event, formatter event.Formatter) Message {
	urgent := map[event.Kind]bool{event.AccountLoginFailed: true, event.ReturnOverdue: true}
	msg := Message{Subject: subject(len(events)), Events: make([]event.View, 0, len(events))}
	items := make([]string, 0, min(len(events), MaxListed))
	for _, ev := range events {
		msg.Events = append(msg.Events, formatter.View(ev))
		msg.Urgent = msg.Urgent || urgent[ev.Kind]
		if len(items) < MaxListed {
			items = append(items, item(ev, formatter))
		}
	}
	separator := "\n"
	if formatter.Full {
		separator = "\n\n"
	}
	msg.Body = strings.Join(items, separator)
	if hidden := len(events) - len(items); hidden > 0 {
		msg.Body += fmt.Sprintf("\n\nand %d more", hidden)
	}
	return msg
}

func NewTestMessage() Message {
	return Message{
		Subject: "test message",
		Body:    "If you can read this, sei-monitor can reach you on this channel.",
		Events:  []event.View{},
		Test:    true,
	}
}

func item(ev event.Event, formatter event.Formatter) string {
	lines := []string{formatter.Title(ev)}
	if formatter.Full || ev.Kind == event.AccountLoginFailed {
		for _, detail := range formatter.Details(ev) {
			lines = append(lines, "  "+detail)
		}
	}
	return strings.Join(lines, "\n")
}

func subject(count int) string {
	if count == 1 {
		return "1 update on your processes"
	}
	return fmt.Sprintf("%d updates on your processes", count)
}

func cut(text string, limit int) string {
	if len(text) <= limit {
		return text
	}
	const marker = "\n…"
	end := limit - len(marker)
	for end > 0 && !utf8.RuneStart(text[end]) {
		end--
	}
	return text[:end] + marker
}
