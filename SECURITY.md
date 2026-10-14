# Security Policy

## Supported Versions

Codescribe supports the newest published release only. Security fixes land in
source first and reach users in the next published release, which installed
copies receive through the in-app updater.

| Version                                                                             | Supported                                        |
| ----------------------------------------------------------------------------------- | ------------------------------------------------ |
| [Latest published release](https://github.com/vetcoders/codescribe/releases/latest) | Yes                                              |
| Unreleased source (version in `Cargo.toml` above the latest release)                | Reports accepted; fixes ship in the next release |
| Older releases                                                                      | No — update to the latest release                |

## Reporting a Vulnerability

If you discover a security issue, please do not open a public issue.

Use one of these private channels:

- Email: `security@vetcoders.io`
- Fallback: `hello@vetcoders.io`

Please include:

- Affected version
- Reproduction steps
- Impact assessment
- Any suggested mitigation

## Response Targets

- Initial acknowledgment: within 2 business days
- Triage decision: within 5 business days
- Fix timeline: shared after triage
- Public disclosure: after mitigation is available

## Disclosure Process

1. We acknowledge and triage the report.
2. We reproduce and classify severity.
3. We implement and validate a fix.
4. We publish release notes and attribution (if requested).

## Scope Notes

For local-first workflows, this project handles:

- Microphone/audio capture
- Local model execution
- Optional API traffic to configured LLM providers
- The license-key service at `https://codescribe.vetcoders.io/api/license/issue`
- The signed Sparkle update feed at `https://codescribe.vetcoders.io/appcast.xml`

Please include provider and endpoint details in your report when relevant.
