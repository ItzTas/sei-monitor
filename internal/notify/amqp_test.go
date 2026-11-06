package notify_test

import (
	"encoding/json"
	"fmt"
	"net/url"
	"os"
	"strings"
	"testing"
	"time"

	amqp "github.com/rabbitmq/amqp091-go"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/notify"
)

type testBroker struct {
	url      string
	exchange string
	channel  *amqp.Channel
}

func newTestBroker(t *testing.T) *testBroker {
	t.Helper()
	address := os.Getenv("TEST_AMQP_URL")
	if address == "" && os.Getenv("CI") != "" {
		t.Fatal("RabbitMQ is required when CI is set: set TEST_AMQP_URL")
	}
	if address == "" {
		t.Skip("RabbitMQ unavailable: set TEST_AMQP_URL, such as amqp://guest:guest@localhost:5672/")
	}
	conn, err := amqp.Dial(address)
	if err != nil {
		t.Fatalf("connect to the test broker: %v", err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	channel, err := conn.Channel()
	if err != nil {
		t.Fatalf("open a channel on the test broker: %v", err)
	}
	broker := &testBroker{url: address, exchange: fmt.Sprintf("sei-monitor-test-%d", time.Now().UnixNano()), channel: channel}
	t.Cleanup(func() { _ = channel.ExchangeDelete(broker.exchange, false, false) })
	return broker
}

func (b *testBroker) sender(address string) *notify.AMQP {
	return notify.NewAMQP(config.AMQP{URL: config.NewSecret(address), Exchange: b.exchange})
}

func (b *testBroker) queue(t *testing.T, keys ...string) string {
	t.Helper()
	if err := b.channel.ExchangeDeclare(b.exchange, amqp.ExchangeTopic, true, false, false, false, nil); err != nil {
		t.Fatalf("declare the exchange: %v", err)
	}
	queue, err := b.channel.QueueDeclare("", false, true, true, false, nil)
	if err != nil {
		t.Fatalf("declare a queue: %v", err)
	}
	for _, key := range keys {
		if err := b.channel.QueueBind(queue.Name, key, b.exchange, false, nil); err != nil {
			t.Fatalf("bind %s: %v", key, err)
		}
	}
	return queue.Name
}

func (b *testBroker) drain(t *testing.T, queue string) []amqp.Delivery {
	t.Helper()
	var deliveries []amqp.Delivery
	for {
		delivery, ok, err := b.channel.Get(queue, true)
		if err != nil {
			t.Fatalf("read %s: %v", queue, err)
		}
		if !ok {
			return deliveries
		}
		deliveries = append(deliveries, delivery)
	}
}

func TestAMQPPublishesEachEventUnderItsKind(t *testing.T) {
	broker := newTestBroker(t)
	everything := broker.queue(t, "#")
	overdue := broker.queue(t, string(event.ReturnOverdue))
	events := []event.Event{
		activity("00000.000001/2026-00", t0),
		{Kind: event.ReturnOverdue, DetectedAt: t0, Payload: event.Payload{ProcessNumber: "00000.000002/2026-00", ReturnDate: "2026-11-03"}},
	}
	events[0].ID, events[1].ID = 7, 8
	msg := notify.Compose(events, event.Formatter{Location: time.UTC})

	err := broker.sender(broker.url).Send(t.Context(), msg)

	if err != nil {
		t.Fatalf("Send = %v", err)
	}
	all := broker.drain(t, everything)
	if len(all) != 2 {
		t.Fatalf("the queue bound to # got %d messages, want 2", len(all))
	}
	for i, delivery := range all {
		view := msg.Events[i]
		body, err := json.Marshal(view)
		if err != nil {
			t.Fatal(err)
		}
		if delivery.RoutingKey != string(view.Kind) || delivery.Type != string(view.Kind) || delivery.MessageId != fmt.Sprint(view.ID) {
			t.Errorf("message %d: routing key %q, type %q, id %q", i, delivery.RoutingKey, delivery.Type, delivery.MessageId)
		}
		if delivery.ContentType != "application/json" || delivery.DeliveryMode != amqp.Persistent || string(delivery.Body) != string(body) {
			t.Errorf("message %d: content type %q, delivery mode %d, body %s", i, delivery.ContentType, delivery.DeliveryMode, delivery.Body)
		}
	}
	if got := broker.drain(t, overdue); len(got) != 1 || got[0].MessageId != "8" {
		t.Errorf("the queue bound to return_overdue got %d messages: %+v", len(got), got)
	}
}

func TestAMQPPublishesTheTestMessageUnderTest(t *testing.T) {
	broker := newTestBroker(t)
	queue := broker.queue(t, notify.TestRoutingKey)

	err := broker.sender(broker.url).Send(t.Context(), notify.NewTestMessage())

	got := broker.drain(t, queue)
	if err != nil || len(got) != 1 || string(got[0].Body) != `{"test":true}` || got[0].ContentType != "application/json" {
		t.Fatalf("Send = %v, messages %+v", err, got)
	}
}

func TestAMQPErrorsLeaveThePasswordOut(t *testing.T) {
	broker := newTestBroker(t)
	wrongPassword, err := url.Parse(broker.url)
	if err != nil {
		t.Fatal(err)
	}
	wrongPassword.User = url.UserPassword(wrongPassword.User.Username(), "s3cr3t")
	unreachable, err := url.Parse(unreachableURL(t))
	if err != nil {
		t.Fatal(err)
	}
	if err := broker.channel.ExchangeDeclare(broker.exchange, amqp.ExchangeFanout, false, false, false, false, nil); err != nil {
		t.Fatalf("declare a fanout exchange: %v", err)
	}
	tests := map[string]struct {
		url  string
		want string
	}{
		"wrong password":         {wrongPassword.String(), "could not connect to the broker: "},
		"unreachable":            {"amqp://guest:s3cr3t@" + unreachable.Host + "/", "could not connect to the broker: "},
		"exchange of other type": {broker.url, "could not declare the exchange " + broker.exchange + ": "},
	}
	for name, tt := range tests {
		err := broker.sender(tt.url).Send(t.Context(), notify.NewTestMessage())
		if err == nil || !strings.HasPrefix(err.Error(), tt.want) || strings.Contains(err.Error(), "s3cr3t") {
			t.Errorf("%s: error = %v", name, err)
		}
	}
}
