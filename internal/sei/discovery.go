package sei

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"sync"
)

var errNotModule = errors.New("the address did not answer like mod-wssei")

func Discover(ctx context.Context, seiURL string, opts Options) (string, error) {
	candidates := []string{
		seiURL + "/sei/modulos/wssei/controlador_ws.php/api/v2",
		seiURL + "/sei/modulos/mod-wssei/controlador_ws.php/api/v2",
	}
	client := opts.apiClient()
	results := make([]error, len(candidates))
	var wg sync.WaitGroup
	for i, base := range candidates {
		wg.Go(func() { results[i] = probe(ctx, client, opts.UserAgent, base) })
	}
	wg.Wait()

	for i, err := range results {
		if err == nil {
			return candidates[i], nil
		}
	}
	for i, err := range results {
		if errors.Is(err, ErrModuleUnavailable) {
			return candidates[i], err
		}
	}
	for _, err := range results {
		if !errors.Is(err, errNotModule) {
			return "", err
		}
	}
	return "", fmt.Errorf("%w: tried %s and %s; set SEI_API_URL if the module lives elsewhere", ErrModuleNotFound, candidates[0], candidates[1])
}

func probe(ctx context.Context, client *http.Client, userAgent, base string) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, base+routeVersion, nil)
	if err != nil {
		return fmt.Errorf("sei: build the %s call: %w", routeVersion, withoutQuery(err))
	}
	req.Header.Set("Accept", "application/json")
	req.Header.Set("User-Agent", userAgent)
	resp, err := client.Do(req)
	if err != nil {
		return fmt.Errorf("sei: look for mod-wssei: %w", withoutQuery(err))
	}
	defer func() { _ = resp.Body.Close() }()

	env, ok, err := decode(resp)
	if err != nil {
		return fmt.Errorf("sei: read the %s answer: %w", routeVersion, err)
	}
	if !ok {
		return errNotModule
	}
	if *env.Success {
		return nil
	}
	apiErr := newAPIError(routeVersion, resp.StatusCode, env.message())
	if errors.Is(apiErr, ErrModuleUnavailable) {
		return apiErr
	}
	return nil
}
