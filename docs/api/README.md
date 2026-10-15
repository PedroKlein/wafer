# WAFER Control Plane API

Machine-readable files for the runtime's HTTP control plane. The reference
for every endpoint, request, response and status code is
[`docs/interfaces/http-api.md`](../interfaces/http-api.md).

- **`openapi.yaml`**: OpenAPI 3.1 description of every route.
- **`bruno-collection/`**: [Bruno](https://www.usebruno.com/) requests for
  local testing.

## Use the Bruno collection

1. Start a pipeline with the control plane enabled:

   ```bash
   cargo run -p wafer-runtime -- --config examples/dag-passthrough-with-api.toml
   ```

2. In Bruno, choose "Open Collection", select `docs/api/bruno-collection/`,
   and pick the `local` environment. Its `baseUrl` is
   `http://localhost:9090`. `metricsUrl` (`http://localhost:9091`) is used
   only when the runtime runs with `--metrics-bind 127.0.0.1:9091`.
3. Select a request, such as **Health/Health Check**, and send it.

The node requests default to the `passthrough` node of that example.

## View the OpenAPI file in Swagger UI

```bash
docker run -p 8080:8080 -e SWAGGER_JSON=/api/openapi.yaml \
  -v "$(pwd)/docs/api:/api" swaggerapi/swagger-ui
```

Then open <http://localhost:8080>.
