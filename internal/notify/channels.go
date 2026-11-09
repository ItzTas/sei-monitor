package notify

import "github.com/ItzTas/sei-monitor/internal/core"

func Channels(cfg core.Channels, userAgent string) []Channel {
	candidates := []struct {
		on    bool
		build func() Channel
	}{
		{cfg.Email != nil, func() Channel { return NewEmail(*cfg.Email, userAgent, nil) }},
		{cfg.Telegram != nil, func() Channel { return NewTelegram(*cfg.Telegram, TelegramAPI, userAgent) }},
		{cfg.Ntfy != nil, func() Channel { return NewNtfy(*cfg.Ntfy, userAgent) }},
		{cfg.Webhook != nil, func() Channel { return NewWebhook(*cfg.Webhook, userAgent) }},
		{cfg.AMQP != nil, func() Channel { return NewAMQP(*cfg.AMQP) }},
	}
	var channels []Channel
	for _, candidate := range candidates {
		if candidate.on {
			channels = append(channels, candidate.build())
		}
	}
	return channels
}
