package notify

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"strconv"
	"time"

	amqp "github.com/rabbitmq/amqp091-go"

	"github.com/ItzTas/sei-monitor/internal/config"
)

const (
	TestRoutingKey = "test"
	amqpTimeout    = 30 * time.Second
)

type AMQP struct {
	url      config.Secret
	exchange string
}

type amqpPublishing struct {
	key     string
	message amqp.Publishing
}

func NewAMQP(cfg config.AMQP) *AMQP {
	return &AMQP{url: cfg.URL, exchange: cfg.Exchange}
}

func (a *AMQP) Name() string {
	return "amqp"
}

func (a *AMQP) Send(ctx context.Context, msg Message) error {
	publishings, err := amqpPublishings(msg)
	if err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(ctx, amqpTimeout)
	defer cancel()
	settings := amqp.Config{Dial: dialWithin(ctx), Properties: amqp.NewConnectionProperties()}
	settings.Properties.SetClientConnectionName("sei-monitor")
	conn, err := amqp.DialConfig(a.url.Reveal(), settings)
	if err != nil {
		return fmt.Errorf("could not connect to the broker: %w", err)
	}
	defer func() { _ = conn.Close() }()
	stop := context.AfterFunc(ctx, func() { _ = conn.CloseDeadline(time.Now()) })
	defer stop()
	channel, err := conn.Channel()
	if err != nil {
		return fmt.Errorf("could not open a channel: %w", err)
	}
	if err := channel.Confirm(false); err != nil {
		return fmt.Errorf("could not turn on publisher confirms: %w", err)
	}
	if err := channel.ExchangeDeclare(a.exchange, amqp.ExchangeTopic, true, false, false, false, nil); err != nil {
		return fmt.Errorf("could not declare the exchange %s: %w", a.exchange, err)
	}
	confirmations := make([]*amqp.DeferredConfirmation, 0, len(publishings))
	for _, p := range publishings {
		confirmation, err := channel.PublishWithDeferredConfirmWithContext(ctx, a.exchange, p.key, false, false, p.message)
		if err != nil {
			return fmt.Errorf("could not publish: %w", err)
		}
		confirmations = append(confirmations, confirmation)
	}
	return awaitConfirmations(ctx, confirmations)
}

func amqpPublishings(msg Message) ([]amqpPublishing, error) {
	if msg.Test {
		return []amqpPublishing{{key: TestRoutingKey, message: jsonPublishing([]byte(`{"test":true}`))}}, nil
	}
	publishings := make([]amqpPublishing, 0, len(msg.Events))
	for _, view := range msg.Events {
		body, err := json.Marshal(view)
		if err != nil {
			return nil, fmt.Errorf("could not encode the event: %w", err)
		}
		message := jsonPublishing(body)
		message.MessageId, message.Type = strconv.FormatInt(view.ID, 10), string(view.Kind)
		publishings = append(publishings, amqpPublishing{key: string(view.Kind), message: message})
	}
	return publishings, nil
}

func jsonPublishing(body []byte) amqp.Publishing {
	return amqp.Publishing{ContentType: "application/json", DeliveryMode: amqp.Persistent, Body: body}
}

func awaitConfirmations(ctx context.Context, confirmations []*amqp.DeferredConfirmation) error {
	refused := 0
	for _, confirmation := range confirmations {
		acked, err := confirmation.WaitContext(ctx)
		if err != nil {
			return fmt.Errorf("the broker did not confirm the messages in time: %w", err)
		}
		if !acked {
			refused++
		}
	}
	if refused > 0 {
		return fmt.Errorf("the broker did not take %d of %d messages", refused, len(confirmations))
	}
	return nil
}

func dialWithin(ctx context.Context) func(network, addr string) (net.Conn, error) {
	return func(network, addr string) (net.Conn, error) {
		conn, err := new(net.Dialer).DialContext(ctx, network, addr)
		if err != nil {
			return nil, err
		}
		deadline, _ := ctx.Deadline()
		if err := conn.SetDeadline(deadline); err != nil {
			return nil, errors.Join(err, conn.Close())
		}
		return conn, nil
	}
}
