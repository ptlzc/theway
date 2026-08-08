# Bedrock SigV4 + Vertex AI ADC

> Parent: master roadmap issue.
> Tier: 5 (auth / cloud).

## Goal

Enterprise users sit behind AWS Bedrock (Claude via SigV4) and GCP Vertex AI (Gemini /
Anthropic Claude via Application Default Credentials). `theway-llm-provider` should support both as first
class providers.

Ship:

- `bedrock` provider — Anthropic on AWS Bedrock; SigV4 request signing.
- `vertex` provider — Gemini *and* Anthropic on GCP Vertex; ADC credential chain.
- Model catalog entries with provider-specific routing (region, endpoint, model id mapping).
- Provider-agnostic streaming behavior identical to direct Anthropic / Google.

## Architecture

```
theway-llm-provider/src/providers/
  bedrock.rs     SigV4 signing via `aws-sigv4` crate; reuses `anthropic` request/response
                  shapes via a thin adapter
  vertex.rs      ADC chain via `gcp_auth` crate; switches model id to vertex/anthropic shape
                  per `model.api`
```

The two providers share most code with their direct counterparts. Differences:

- **Bedrock**: SigV4 signing per request, region-specific endpoint, model-id mapping
  (`anthropic.claude-sonnet-4-20250514-v1:0` etc.), `accept`/`content-type` headers.
- **Vertex**: ADC chain (service account JSON, gcloud-cached, GCE metadata server); endpoint
  `{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/{publisher}/models/{model}:streamGenerateContent`.

Credential resolution:

- Bedrock: `AWS_REGION`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE`, or
  EC2/EKS instance role — handled by `aws-config`.
- Vertex: `GOOGLE_APPLICATION_CREDENTIALS` env var, gcloud cached creds, GCE metadata —
  handled by `gcp_auth`.

Model catalog (`models.generated.rs` analogue) gets per-provider entries. The generator script
includes Bedrock + Vertex model lists.

## Stability

- Credential expiry: both AWS SigV4 (STS) and Vertex ADC tokens expire. Refresh proactively;
  retry the request if a 401/403 returns *after* refresh fails.
- Region/endpoint validation at startup; if region is missing, fail with a clear actionable
  error.
- Streaming over Vertex uses `streamGenerateContent` with newline-delimited JSON; parser must
  handle partial frames across TCP packets.
- 429 / 5xx retries reuse the provider HTTP retry helper already in `theway-llm-provider`.

## Extensibility

- Provider definition trait stays unchanged — Bedrock/Vertex are just two more impls.
- Region selection: per-call override via request option; default per-provider config.

## Performance

- Signing cost (Bedrock) is O(1) per request; cached SigV4 signer kept on the client.
- ADC refresh runs once per credential lifetime (~1h), not per request.

## Testing

| Layer | What |
|---|---|
| **unit** | SigV4 signature parity against AWS-provided test vectors. Vertex endpoint URL construction for various model id shapes. Streaming JSON-NDJSON parser handles split frames. |
| **integration** | `wiremock` faking Bedrock + Vertex streaming endpoints; assert end-to-end request shape, including correct headers / auth. |
| **e2e** | Skip in default CI (needs real cloud creds). Gated under `BEDROCK_E2E=1`/`VERTEX_E2E=1`; runs a real streaming roundtrip when set. |

## Acceptance criteria

- `theway --provider bedrock --model anthropic.claude-sonnet-4` works with AWS creds in env or
  via `--profile`.
- `theway --provider vertex --model claude-sonnet-4@gcp` works with ADC.
- Streaming output indistinguishable from direct Anthropic / Google in shape.
- Retries on 429/5xx use exponential backoff (provider helper).

## Out of scope

- Azure OpenAI (separate provider issue if requested).
- Per-request region failover.
- Cross-region traffic optimization.
- See [[12-login-oauth]] for non-cloud-IAM auth.
