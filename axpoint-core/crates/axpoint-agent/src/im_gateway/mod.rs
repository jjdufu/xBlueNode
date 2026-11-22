//! Zero-inbound-port enterprise IM outbound bidirectional WebSocket/Stream gateway (`07-STEP21`).

pub mod chunker;
pub mod dingtalk_stream;
pub mod feishu_ws;
pub mod protocol;
pub mod supervisor;
pub mod wecom_bot;

pub use chunker::{
    IM_MARKDOWN_MAX_CHUNK_BYTES, IM_PROGRESS_THROTTLE_MS, ProgressThrottler,
    progress_hint_for_event, split_markdown_chunks,
};
pub use dingtalk_stream::{
    decode_dingtalk_frame, encode_dingtalk_ping, encode_dingtalk_reply, resolve_dingtalk_dial_plan,
};
pub use feishu_ws::{
    decode_feishu_frame, encode_feishu_ping, encode_feishu_reply, resolve_feishu_dial_plan,
};
pub use protocol::{
    DecodedPlatformFrame, ImSocketStream, ImWsStream, PlatformDialPlan, build_im_http_client,
    dial_websocket, normalize_url_for_guard, sanitize_endpoint_preview, strip_leading_bot_mention,
};
pub use supervisor::{
    DEFAULT_MAX_IM_SLOTS, IM_HEARTBEAT_INTERVAL, IM_LEASE_DURATION_SECS,
    IM_SUPERVISOR_SCAN_INTERVAL, ImGatewayDeps, ImGatewaySupervisor, generate_im_bind_token,
    probe_im_connector,
};
pub use wecom_bot::{
    decode_wecom_frame, encode_wecom_ping, encode_wecom_reply, resolve_wecom_dial_plan,
};
