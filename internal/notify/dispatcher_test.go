package notify_test

import (
	"context"
	"errors"
	"log/slog"
	"slices"
	"sync"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/core/coretest"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/modules/deliveries"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/notify"
)

type fakeChannel struct {
	name string
	mu   sync.Mutex
	got  []notify.Message
	errs []error
	sent chan notify.Message
	send func(ctx context.Context) error
}

func (c *fakeChannel) Name() string {
	return c.name
}

func (c *fakeChannel) Send(ctx context.Context, msg notify.Message) error {
	c.mu.Lock()
	c.got = append(c.got, msg)
	var err error
	if len(c.errs) > 0 {
		err, c.errs = c.errs[0], c.errs[1:]
	}
	c.mu.Unlock()
	if c.sent != nil {
		c.sent <- msg
	}
	if c.send != nil {
		return c.send(ctx)
	}
	return err
}

func (c *fakeChannel) messages() []notify.Message {
	c.mu.Lock()
	defer c.mu.Unlock()
	return slices.Clone(c.got)
}

type outbox struct {
	*notify.Postgres
	cycles  *monitor.Postgres
	listing *deliveries.Postgres
	account core.Account
}

func newOutbox(t *testing.T) outbox {
	t.Helper()
	database := coretest.Open(t)
	cycles := monitor.NewPostgres(database.Pool)
	account, err := cycles.EnsureAccount(t.Context(), core.AccountKey{SEIBaseURL: "https://sei.example.org", Username: "fulano.tal", OrgID: 63}, t0)
	if err != nil {
		t.Fatal(err)
	}
	return outbox{Postgres: notify.NewPostgres(database.Pool), cycles: cycles, listing: deliveries.NewPostgres(database.Pool), account: account}
}

func (o outbox) add(t *testing.T, at time.Time, events ...event.Event) {
	t.Helper()
	changes := monitor.CycleChanges{AccountID: o.account.ID, Now: at, Events: events, Channels: []string{"telegram", "webhook"}}
	if _, err := o.cycles.ApplyCycle(t.Context(), changes); err != nil {
		t.Fatal(err)
	}
}

func (o outbox) deliveries(t *testing.T, channel string) []core.Delivery {
	t.Helper()
	list, err := o.listing.ListDeliveries(t.Context(), deliveries.Filter{AccountID: o.account.ID, Channel: channel, Limit: 100})
	if err != nil {
		t.Fatal(err)
	}
	return list
}

func newDispatcher(o outbox, now *time.Time, channels ...notify.Channel) *notify.Dispatcher {
	formatter := func() event.Formatter { return event.Formatter{Location: time.UTC} }
	clock := notify.WithClock(func() time.Time { return *now }, time.After)
	return notify.NewDispatcher(o, channels, formatter, slog.New(slog.DiscardHandler), clock)
}

func TestOneMessagePerChannelWithEverythingDue(t *testing.T) {
	o := newOutbox(t)
	o.add(t, t0, activity("00000.000001/2026-00", t0), activity("00000.000002/2026-00", t0), activity("00000.000003/2026-00", t0))
	now := t0
	telegram, webhook := &fakeChannel{name: "telegram"}, &fakeChannel{name: "webhook"}
	dispatcher := newDispatcher(o, &now, telegram, webhook)

	dispatcher.RunOnce(t.Context())
	now = t0.Add(time.Hour)
	dispatcher.RunOnce(t.Context())

	for _, channel := range []*fakeChannel{telegram, webhook} {
		got := channel.messages()
		if len(got) != 1 || got[0].Subject != "3 updates on your processes" || len(got[0].Events) != 3 {
			t.Errorf("%s got %+v", channel.name, got)
		}
		for _, d := range o.deliveries(t, channel.name) {
			if d.Status != core.DeliverySent || d.Attempts != 1 || !d.SentAt.Equal(t0) {
				t.Errorf("%s delivery = %+v", channel.name, d)
			}
		}
	}
}

func TestFailedDeliveryRetriesOnTheScheduleThenGivesUp(t *testing.T) {
	o := newOutbox(t)
	o.add(t, t0, activity("00000.000001/2026-00", t0))
	failure := errors.New("the server answered 500")
	telegram := &fakeChannel{name: "telegram", errs: slices.Repeat([]error{failure}, 10)}
	now := t0
	dispatcher := newDispatcher(o, &now, telegram)

	attemptAt := t0
	for i, delay := range []time.Duration{0, time.Minute, 5 * time.Minute, 30 * time.Minute, 2 * time.Hour, 6 * time.Hour} {
		attemptAt = attemptAt.Add(delay)
		now = attemptAt.Add(-time.Second)
		dispatcher.RunOnce(t.Context())
		now = attemptAt
		dispatcher.RunOnce(t.Context())
		if got := len(telegram.messages()); got != i+1 {
			t.Fatalf("attempt %d at %s: %d messages so far", i+1, attemptAt, got)
		}
	}
	now = attemptAt.Add(24 * time.Hour)
	dispatcher.RunOnce(t.Context())

	d := o.deliveries(t, "telegram")[0]
	if len(telegram.messages()) != 6 || d.Status != core.DeliveryFailed || d.Attempts != 6 || d.LastError != "the server answered 500" {
		t.Errorf("after giving up: %d messages, delivery %+v", len(telegram.messages()), d)
	}
}

func TestDeliveryThatFailedOnceIsSentOnTheRetry(t *testing.T) {
	o := newOutbox(t)
	o.add(t, t0, activity("00000.000001/2026-00", t0))
	telegram := &fakeChannel{name: "telegram", errs: []error{errors.New("could not reach the server: connection refused")}}
	now := t0
	dispatcher := newDispatcher(o, &now, telegram)

	dispatcher.RunOnce(t.Context())
	now = t0.Add(time.Minute)
	dispatcher.RunOnce(t.Context())

	d := o.deliveries(t, "telegram")[0]
	if d.Status != core.DeliverySent || d.Attempts != 2 || d.LastError != "" || !d.SentAt.Equal(now) {
		t.Errorf("delivery = %+v", d)
	}
}

func TestShutdownDuringASendLeavesTheDeliveriesPending(t *testing.T) {
	o := newOutbox(t)
	o.add(t, t0, activity("00000.000001/2026-00", t0))
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	telegram := &fakeChannel{name: "telegram", send: func(ctx context.Context) error {
		cancel()
		return ctx.Err()
	}}
	now := t0

	newDispatcher(o, &now, telegram).RunOnce(ctx)

	d := o.deliveries(t, "telegram")[0]
	if len(telegram.messages()) != 1 || d.Status != core.DeliveryPending || d.Attempts != 0 || d.LastError != "" {
		t.Errorf("delivery after the shutdown = %+v", d)
	}
}

func TestRunDeliversEveryPollIntervalUntilTheContextEnds(t *testing.T) {
	o := newOutbox(t)
	o.add(t, t0, activity("00000.000001/2026-00", t0))
	telegram := &fakeChannel{name: "telegram", sent: make(chan notify.Message, 1)}
	ticks := make(chan time.Time)
	var waits []time.Duration
	after := func(d time.Duration) <-chan time.Time {
		waits = append(waits, d)
		return ticks
	}
	formatter := func() event.Formatter { return event.Formatter{Location: time.UTC} }
	dispatcher := notify.NewDispatcher(o, []notify.Channel{telegram}, formatter, slog.New(slog.DiscardHandler), notify.WithClock(func() time.Time { return t0 }, after))
	ctx, cancel := context.WithCancel(t.Context())
	done := make(chan error)

	go func() { done <- dispatcher.Run(ctx) }()
	first := <-telegram.sent
	o.add(t, t0, activity("00000.000002/2026-00", t0))
	ticks <- t0
	second := <-telegram.sent
	cancel()
	err := <-done

	if err != nil || first.Events[0].ProcessNumber != "00000.000001/2026-00" || second.Events[0].ProcessNumber != "00000.000002/2026-00" {
		t.Errorf("Run = %v, first %+v, second %+v", err, first.Events, second.Events)
	}
	if !slices.Equal(waits, []time.Duration{notify.PollInterval, notify.PollInterval}) {
		t.Errorf("waits = %v", waits)
	}
}

func TestRunWithoutChannelsReturnsAtOnce(t *testing.T) {
	formatter := func() event.Formatter { return event.Formatter{} }
	dispatcher := notify.NewDispatcher(newOutbox(t), nil, formatter, slog.New(slog.DiscardHandler))

	if err := dispatcher.Run(t.Context()); err != nil {
		t.Errorf("Run = %v", err)
	}
}
