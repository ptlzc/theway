# theway-llm-provider

English | [中文](README.zh.md)

`theway-llm-provider` is the normalized streaming LLM client used by `theway-core`, `theway-daemon`, and protocol-facing model catalogs. Provider-specific HTTP payloads and stream frames enter through one `ApiProvider` implementation and leave as common `AssistantMessageEvent` records.

The crate owns model and image catalogs, provider registration, environment-key lookup, message normalization, SSE and event-stream utilities, and provider protocol implementations. It does not own agent loops, tool execution, session persistence, retries across agent turns, or user interaction.

## Public API

- `stream` and `stream_simple` return an `AssistantMessageEventStream`; `complete` and `complete_simple` collect the same stream into an `AssistantMessage`.
- `ApiProvider` plus `register_api_provider` and `unregister_api_providers` support built-in and runtime-registered protocols.
- `Model`, `Context`, `Message`, `Tool`, `StreamOptions`, `AssistantMessage`, and `AssistantMessageEvent` form the provider-neutral request and response model.
- `ProviderRequestInterceptor` optionally transforms non-secret request headers and provider-format JSON, then observes redacted response metadata or request failure before stream consumption.
- `get_model`, `list_models`, and custom-model registration expose the generated and runtime model catalog.
- `images`, `get_image_model`, and `list_image_models` expose the separate image-generation path.

## Features

Concrete text providers are selected with Cargo features: `anthropic`, `openai-completions`, `openai-responses`, `openai-codex-responses`, `azure-openai-responses`, `google`, `google-vertex`, `amazon-bedrock`, `cloudflare`, `mistral`, and `faux`. `openrouter-images` enables image generation, and `all-providers` enables the complete workspace bundle. The default enables `anthropic` and `faux`.

```bash
cargo check -p theway-llm-provider --no-default-features
cargo check -p theway-llm-provider --all-features
cargo test -p theway-llm-provider --all-features
```

Examples under [`examples/`](examples/anthropic_hello.rs) that contact a provider require the corresponding API key; tests use local or scripted fixtures.

## Documentation

- [Provider pipeline and extension rules](docs/architecture.md)
