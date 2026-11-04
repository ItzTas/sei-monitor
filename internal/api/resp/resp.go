package resp

import (
	"encoding/base64"
	"encoding/json"
	"errors"

	"github.com/danielgtaylor/huma/v2"
)

const SecurityScheme = "bearer"

var ErrInvalidCursor = errors.New("invalid cursor")

type PageInput struct {
	Limit  int    `query:"limit" default:"50" minimum:"1" maximum:"200" doc:"Items per page."`
	Cursor string `query:"cursor" doc:"The next_cursor of the previous page."`
}

type List[T any] struct {
	Items      []T     `json:"items"`
	NextCursor *string `json:"next_cursor" doc:"Pass it as cursor to read the next page; null on the last page."`
}

func Bearer() []map[string][]string {
	return []map[string][]string{{SecurityScheme: {}}}
}

func DecodeCursor[K any](text string) (*K, error) {
	if text == "" {
		return nil, nil
	}
	raw, err := base64.RawURLEncoding.DecodeString(text)
	if err != nil {
		return nil, ErrInvalidCursor
	}
	var key K
	if err := json.Unmarshal(raw, &key); err != nil {
		return nil, ErrInvalidCursor
	}
	return &key, nil
}

func Paginate[R, V, K any](rows []R, limit int, view func(R) V, key func(R) K) (List[V], error) {
	list := List[V]{Items: make([]V, 0, min(len(rows), limit))}
	if len(rows) > limit {
		rows = rows[:limit]
		raw, err := json.Marshal(key(rows[len(rows)-1]))
		if err != nil {
			return List[V]{}, err
		}
		cursor := base64.RawURLEncoding.EncodeToString(raw)
		list.NextCursor = &cursor
	}
	for _, row := range rows {
		list.Items = append(list.Items, view(row))
	}
	return list, nil
}

func Error(err error) error {
	if errors.Is(err, ErrInvalidCursor) {
		return huma.Error400BadRequest("invalid cursor")
	}
	return huma.Error500InternalServerError("internal error", err)
}
