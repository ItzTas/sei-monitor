package notify_test

import (
	"slices"
	"testing"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/notify"
)

func TestChannelsUseTheNamesOfTheirDeliveries(t *testing.T) {
	cfg := config.Channels{
		Email:    &config.Email{Host: "smtp.example.org", Port: 587, From: "a@example.org", To: []string{"b@example.org"}, TLS: config.TLSStartTLS},
		Telegram: &config.Telegram{Token: config.NewSecret(botToken), ChatID: "-1001234567890"},
		Ntfy:     &config.Ntfy{URL: "https://ntfy.sh/sei-topic"},
		Webhook:  &config.Webhook{URL: "https://hooks.example.org/sei", Secret: config.NewSecret(webhookSecret)},
	}

	var names []string
	for _, channel := range notify.Channels(cfg, "sei-monitor/test") {
		names = append(names, channel.Name())
	}

	if !slices.Equal(names, cfg.Enabled()) {
		t.Errorf("channel names = %v, deliveries go to %v", names, cfg.Enabled())
	}
	if channels := notify.Channels(config.Channels{}, "sei-monitor/test"); len(channels) != 0 {
		t.Errorf("channels without configuration = %v", channels)
	}
}
