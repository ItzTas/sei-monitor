package notify

import (
	"context"
	"log/slog"
	"strings"
	"sync"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/event"
)

const (
	BatchSize      = 500
	PollInterval   = 30 * time.Second
	maxErrorLength = 500
)

var retryDelays = []time.Duration{time.Minute, 5 * time.Minute, 30 * time.Minute, 2 * time.Hour, 6 * time.Hour}

type Channel interface {
	Name() string
	Send(ctx context.Context, msg Message) error
}

type Outbox interface {
	DueDeliveries(ctx context.Context, channel string, now time.Time, limit int) ([]core.DueDelivery, error)
	UpdateDeliveries(ctx context.Context, deliveries []core.Delivery) error
}

type Dispatcher struct {
	outbox    Outbox
	channels  []Channel
	formatter func() event.Formatter
	log       *slog.Logger
	now       func() time.Time
	after     func(time.Duration) <-chan time.Time
}

type Option func(*Dispatcher)

func WithClock(now func() time.Time, after func(time.Duration) <-chan time.Time) Option {
	return func(d *Dispatcher) {
		d.now, d.after = now, after
	}
}

func NewDispatcher(outbox Outbox, channels []Channel, formatter func() event.Formatter, log *slog.Logger, options ...Option) *Dispatcher {
	d := &Dispatcher{outbox: outbox, channels: channels, formatter: formatter, log: log, now: time.Now, after: time.After}
	for _, option := range options {
		option(d)
	}
	return d
}

func (d *Dispatcher) Run(ctx context.Context) error {
	if len(d.channels) == 0 {
		return nil
	}
	for {
		d.RunOnce(ctx)
		select {
		case <-ctx.Done():
			return nil
		case <-d.after(PollInterval):
		}
	}
}

func (d *Dispatcher) RunOnce(ctx context.Context) {
	var wg sync.WaitGroup
	for _, channel := range d.channels {
		wg.Go(func() { d.deliver(ctx, channel) })
	}
	wg.Wait()
}

func (d *Dispatcher) deliver(ctx context.Context, channel Channel) {
	name := channel.Name()
	due, err := d.outbox.DueDeliveries(ctx, name, d.now(), BatchSize)
	if err != nil {
		d.log.Error("could not read the pending notifications", "channel", name, "error", err)
		return
	}
	if len(due) == 0 {
		return
	}
	events := make([]event.Event, 0, len(due))
	for _, delivery := range due {
		events = append(events, delivery.Event)
	}
	sendErr := channel.Send(ctx, Compose(events, d.formatter()))
	if sendErr != nil && ctx.Err() != nil {
		return
	}
	now := d.now()
	attempts := make([]core.Delivery, 0, len(due))
	for _, delivery := range due {
		attempts = append(attempts, attempt(delivery.Delivery, sendErr, now))
	}
	recordCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), 10*time.Second)
	defer cancel()
	if err := d.outbox.UpdateDeliveries(recordCtx, attempts); err != nil {
		d.log.Error("could not record the notification attempt", "channel", name, "error", err)
		return
	}
	d.report(name, attempts, sendErr)
}

func (d *Dispatcher) report(channel string, attempts []core.Delivery, sendErr error) {
	if sendErr == nil {
		d.log.Info("notifications sent", "channel", channel, "events", len(attempts))
		return
	}
	gaveUp := 0
	for _, a := range attempts {
		if a.Status == core.DeliveryFailed {
			gaveUp++
		}
	}
	d.log.Warn("could not send the notifications", "channel", channel, "events", len(attempts), "gave_up", gaveUp, "error", sendErr)
}

func attempt(delivery core.Delivery, sendErr error, now time.Time) core.Delivery {
	delivery.Attempts++
	if sendErr == nil {
		delivery.Status, delivery.SentAt, delivery.LastError = core.DeliverySent, now, ""
		return delivery
	}
	delivery.LastError = cut(strings.ToValidUTF8(sendErr.Error(), ""), maxErrorLength)
	if delivery.Attempts > len(retryDelays) {
		delivery.Status = core.DeliveryFailed
		return delivery
	}
	delivery.NextAttemptAt = now.Add(retryDelays[delivery.Attempts-1])
	return delivery
}
