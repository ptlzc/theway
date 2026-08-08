# Multi-modal IO (image input, terminal image rendering)

> Parent: master roadmap issue.
> Tier: 7 (cross-platform & multimodal).
>
> **Note (2026-05-20)**: Windows support was originally bundled here but is explicitly
> de-scoped. `theway` targets Linux + macOS only. Multi-modal remains in scope.

## Goal

Image inputs (CLI flag, REPL paste, `@image.png` mention) and image outputs (image-generation
tool results displayed in supported terminals via Kitty / iTerm2 / Sixel protocols).

## Architecture

```
theway-llm-provider/src/types.rs                    add Image variant to UserContent / ContentBlock
theway-llm-provider/src/providers/{anthropic,openai,google}.rs   image part encoding (base64 / URL)
theway/src/tui/image.rs      detect terminal protocol; render image inline
theway/src/cli.rs            --image PATH flag (repeatable)
theway/src/picker/mentions.rs `@image.png` resolves to an Image part
```

Terminal-protocol detection:

- Kitty (TERM=xterm-kitty or `KITTY_WINDOW_ID`)
- iTerm2 (`TERM_PROGRAM=iTerm.app`)
- Sixel (terminfo query or `XTSMGRAPHICS`)
- Fallback: print a `[image: <path>]` placeholder.

## Stability

- Image input size cap: 10 MiB per image, max 10 images per message.
- Format validation: PNG / JPEG / WebP / GIF only; rejects others with a clear error.
- Provider divergence: each provider supports a subset; if the user attaches an unsupported
  format, return a clear error before sending.

## Extensibility

- `ContentBlock::Image` is the canonical type; providers handle their own encoding.
- Image-rendering protocol detection sits behind a trait; new protocols slot in.

## Performance

- Image input encoding (base64) is done streaming on send; no double buffering of the entire
  file in memory.
- Terminal image rendering caches the encoded form per image to avoid re-encoding on resize.

## Testing

| Layer | What |
|---|---|
| **unit** | Format detection from magic bytes; provider-specific encoders produce known-good byte sequences. |
| **integration** | Send a `Message::User` with an Image part through the faux provider; faux echoes the encoded part; agent loop handles it. |
| **e2e** | `theway --image fixture.png "describe"` with the faux provider asserts the request body contains the image; output rendering on a Kitty-emulating PTY produces inline image escape codes. |

## Acceptance criteria

- `theway --image foo.png "what is this"` sends the image to the model.
- In a Kitty-protocol terminal, an image attachment is rendered inline.
- Unsupported image format produces a clear error, not a panic.

## Out of scope

- Windows support — explicitly de-scoped (Linux + macOS only).
- Video / audio inputs.
- Generated-image saving to disk by default (left to a future image-output handler).
