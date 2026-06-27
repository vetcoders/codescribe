# Attachment & vision input pipeline

How image attachments travel from the chat UI to the model as real vision input.

## Internal representation

- UI attachments are `codescribe_core::attachment::Attachment` (path + kind +
  source + display name). Clipboard, drag&drop and file-picker all produce the
  same `Attachment` and land in `VoiceChatOverlayState::attachments` — the send
  path is **source-agnostic**.
- The model-facing message is `codescribe_core::agent::Message` with a
  `Vec<ContentBlock>`. Images are `ContentBlock::Image { data, media_type }`
  (raw bytes + MIME), converted to provider JSON in
  `app/agent/openai_provider.rs` as an `input_image` data URI.

## Flow

1. On send, `build_attachments_block` (`app/ui/voice_chat/api/send.rs`) inlines
   text files and appends image **paths** under the
   `ATTACHMENTS (image paths)` marker. It only lists an image (and only then
   claims "will be sent as vision input") when the file is a vision-supported
   format within `attachment::MAX_VISION_IMAGE_BYTES`. Oversized / unsupported
   images get an honest message and are **not** listed.
2. The payload is a single `String` (the send callback is `Fn(String)`).
3. Agent path: `run_agent_send_path` (`app/controller/helpers.rs`) calls
   `build_image_attachments_from_text`, which strips the marker block from the
   text and loads each listed image into an `ImageAttachment` via
   `attachment::load_image_for_vision`. These go to `AgentSession::send` as real
   image blocks. Images that fail to load are reported to the user (not silently
   dropped) and the cap is `MAX_AGENT_VISION_IMAGES`.
4. Legacy path: `ai_formatting::build_responses_user_content` uses the same
   `attachment::parse_image_attachment_block` + `load_image_for_vision` contract,
   so both routes behave identically.

## Image support / capability

The assistive backend is assumed to be vision-capable (OpenAI Responses
`input_image`). There is no per-model capability probe — if a future text-only
backend is introduced, gate image forwarding here and surface a clear
"backend does not support image analysis" message instead of attaching.

## Marker contract (single source of truth)

`core/attachment.rs` owns the marker constant and parsing:
`IMAGE_PATHS_MARKER`, `parse_image_attachment_block`, `image_media_type`,
`load_image_for_vision`, `MAX_VISION_IMAGE_BYTES`. Both send paths depend on it;
change the marker in one place only.

## Adding another image-capable provider

Implement `AgentProvider::build_image_block` for the provider (see
`openai_provider.rs`) to emit its native image block; the rest of the pipeline
(`ContentBlock::Image`) is provider-agnostic.
