//! Public provider construction must validate the supplied authority before
//! canonicalizing the API path. These tests perform no HTTP or settings I/O.

use codescribe_core::llm::provider::{CustomProvider, ProviderError, WireFamily};

const WIRES: [WireFamily; 2] = [WireFamily::OpenAiResponses, WireFamily::AnthropicMessages];

#[test]
fn missing_authorities_are_not_repaired_into_hosts() {
    for endpoint in [
        "https:///v1/messages",
        "http:///v1/responses",
        "https:////proxy.example/v1/messages",
        "https:/proxy.example/v1/messages",
        "https:proxy.example/v1/messages",
        "https:proxy.example/path://other.example",
        "https:/proxy.example/path://other.example",
        "http:/localhost:8080",
        "https://",
        "http://",
        "https://?tenant=fixture",
        "https://#section",
        r"https:\\proxy.example\v1\messages",
    ] {
        assert_invalid_endpoint(endpoint);
    }
}

#[test]
fn malformed_authorities_are_rejected_before_endpoint_normalization() {
    for endpoint in [
        "https://:443/v1/messages",
        "https://user@/v1/messages",
        "https://[::1/v1/messages",
        "https://[not-an-ip]/v1/messages",
        "https://proxy.example:not-a-port/v1/messages",
        "https://proxy.example:65536/v1/messages",
        "https://proxy example/v1/messages",
        "https://%zz/v1/messages",
    ] {
        assert_invalid_endpoint(endpoint);
    }
}

fn assert_invalid_endpoint(endpoint: &str) {
    for wire in WIRES {
        assert_eq!(
            CustomProvider::new("Endpoint fixture", wire, endpoint),
            Err(ProviderError::InvalidEndpoint(endpoint.to_string())),
            "{wire:?} must reject the original malformed endpoint: {endpoint:?}"
        );
    }
}

#[test]
fn valid_authorities_keep_their_origin_and_normalize_each_wire_idempotently() {
    // Expected origins are literals, independent of the production parser.
    // Each row supplies input, normalized origin/mount, and query/fragment.
    for (endpoint, base, tail) in [
        ("https://proxy.example", "https://proxy.example", ""),
        ("http://localhost:8080", "http://localhost:8080", ""),
        ("http://127.0.0.1:8080/v1", "http://127.0.0.1:8080", ""),
        ("http://[::1]:8080", "http://[::1]:8080", ""),
        (
            "HTTPS://PROXY.EXAMPLE/v1/messages",
            "https://proxy.example",
            "",
        ),
        (
            "  https://proxy.example/mount/v1/chat/completions/  ",
            "https://proxy.example/mount",
            "",
        ),
        (
            "https://proxy.example:9443/mount/v1/messages",
            "https://proxy.example:9443/mount",
            "",
        ),
        (
            "https://proxy.example/mount/v1/responses?next=/v1/messages#section",
            "https://proxy.example/mount",
            "?next=/v1/messages#section",
        ),
    ] {
        for wire in WIRES {
            let row = CustomProvider::new("Endpoint fixture", wire, endpoint)
                .unwrap_or_else(|error| panic!("{wire:?} refused {endpoint:?}: {error}"));
            let suffix = match wire {
                WireFamily::OpenAiResponses => "/v1/responses",
                WireFamily::AnthropicMessages => "/v1/messages",
            };
            assert_eq!(row.endpoint, format!("{base}{suffix}{tail}"));
            assert_eq!(row.wire, wire);
            assert_eq!(
                CustomProvider::new("Endpoint fixture", wire, &row.endpoint).unwrap(),
                row,
                "normalizing an accepted endpoint again must preserve its identity"
            );
        }
    }
}
