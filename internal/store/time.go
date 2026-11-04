package store

import (
	"database/sql/driver"
	"fmt"
	"time"
)

const timeLayout = "2006-01-02T15:04:05.000000Z07:00"

type dbTime time.Time

func (t dbTime) Value() (driver.Value, error) {
	value := time.Time(t)
	if value.IsZero() {
		return nil, nil
	}
	return value.UTC().Truncate(time.Microsecond).Format(timeLayout), nil
}

func (t *dbTime) Scan(src any) error {
	switch value := src.(type) {
	case nil:
		*t = dbTime{}
	case time.Time:
		*t = dbTime(value.UTC())
	case string:
		return t.parse(value)
	case []byte:
		return t.parse(string(value))
	default:
		return fmt.Errorf("store: unexpected time value of type %T", src)
	}
	return nil
}

func (t *dbTime) parse(text string) error {
	parsed, err := time.Parse(time.RFC3339Nano, text)
	if err != nil {
		return fmt.Errorf("store: read a stored time: %w", err)
	}
	*t = dbTime(parsed.UTC())
	return nil
}
