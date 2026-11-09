package notify_test

import (
	"slices"
	"testing"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/notify"
)

func TestChannelsUseTheNamesOfTheirDeliveries(t *testing.T) {
	cfg := core.Channels{
		Email:    &core.Email{Host: "smtp.example.org", Port: 587, From: "a@example.org", To: []string{"b@example.org"}, TLS: core.TLSStartTLS},
		Telegram: &core.Telegram{Token: core.NewSecret(botToken), ChatID: "-1001234567890"},
		Ntfy:     &core.Ntfy{URL: "https://ntfy.sh/sei-topic"},
		Webhook:  &core.Webhook{URL: "https://hooks.example.org/sei", Secret: core.NewSecret(webhookSecret)},
		AMQP:     &core.AMQP{URL: core.NewSecret("amqp://monitor:pw@rabbitmq:5672/"), Exchange: core.DefaultAMQPExchange},
	}

	var names []string
	for _, channel := range notify.Channels(cfg, "sei-monitor/test") {
		names = append(names, channel.Name())
	}

	if !slices.Equal(names, cfg.Enabled()) {
		t.Errorf("channel names = %v, deliveries go to %v", names, cfg.Enabled())
	}
	if channels := notify.Channels(core.Channels{}, "sei-monitor/test"); len(channels) != 0 {
		t.Errorf("channels without configuration = %v", channels)
	}
}
